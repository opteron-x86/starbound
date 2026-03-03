// file: crates/game/src/persistence.rs
//! SQLite persistence layer.
//!
//! The save game IS a SQLite file. Galaxy state lives in normalized
//! tables (queryable by the simulation and encounter pipeline).
//! The journey (player + crew + threads) is stored as a JSON document
//! — it's complex, changes shape as the game evolves, and is always
//! loaded in full.
//!
//! Design principle: galaxy state is the world's truth, queried often.
//! Journey state is the player's truth, loaded once per session.
//!
//! Schema note: the `civilizations` table stores macro-layer polities
//! (Civilization structs). The `factions` table stores meso-layer
//! factions (Faction structs) — guilds, military branches, criminal
//! networks, religious orders. These are distinct entity types that
//! interact through influence maps and faction_presence on systems.

use std::path::Path;

use rusqlite::{params, Connection, Result as SqlResult};
use uuid::Uuid;

use starbound_core::galaxy::*;
use starbound_core::journey::Journey;

/// A handle to an open save file.
pub struct SaveFile {
    conn: Connection,
}

impl SaveFile {
    /// Create a new save file at the given path and initialize the schema.
    pub fn create(path: &Path) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        let save = Self { conn };
        save.init_schema()?;
        Ok(save)
    }

    /// Open an existing save file.
    pub fn open(path: &Path) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        Ok(Self { conn })
    }

    /// Open an in-memory database (for tests).
    pub fn in_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        let save = Self { conn };
        save.init_schema()?;
        Ok(save)
    }

    fn init_schema(&self) -> SqlResult<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS sectors (
                id          TEXT PRIMARY KEY,
                data_json   TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS star_systems (
                id                  TEXT PRIMARY KEY,
                name                TEXT NOT NULL,
                sector_id           TEXT,
                pos_x               REAL NOT NULL,
                pos_y               REAL NOT NULL,
                star_type           TEXT NOT NULL,
                controlling_civ     TEXT,
                infrastructure      TEXT NOT NULL,
                data_json           TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS civilizations (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                data_json   TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS factions (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                category    TEXT NOT NULL,
                data_json   TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS connections (
                system_a    TEXT NOT NULL,
                system_b    TEXT NOT NULL,
                distance_ly REAL NOT NULL,
                route_type  TEXT NOT NULL,
                data_json   TEXT NOT NULL,
                PRIMARY KEY (system_a, system_b)
            );

            CREATE TABLE IF NOT EXISTS journey (
                id          INTEGER PRIMARY KEY CHECK (id = 1),
                data_json   TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_systems_civ
                ON star_systems(controlling_civ);
            CREATE INDEX IF NOT EXISTS idx_systems_sector
                ON star_systems(sector_id);
            CREATE INDEX IF NOT EXISTS idx_factions_category
                ON factions(category);
            ",
        )
    }

    // -------------------------------------------------------------------
    // Galaxy writes
    // -------------------------------------------------------------------

    pub fn save_sector(&self, sector: &Sector) -> SqlResult<()> {
        let json = serde_json::to_string(sector).expect("Sector serialization");
        self.conn.execute(
            "INSERT OR REPLACE INTO sectors (id, data_json) VALUES (?1, ?2)",
            params![sector.id.to_string(), json],
        )?;
        Ok(())
    }

    pub fn save_system(&self, system: &StarSystem, sector_id: Option<Uuid>) -> SqlResult<()> {
        let json = serde_json::to_string(system).expect("StarSystem serialization");
        let civ_str = system.controlling_civ.map(|id| id.to_string());
        let sector_str = sector_id.map(|id| id.to_string());
        self.conn.execute(
            "INSERT OR REPLACE INTO star_systems
                (id, name, sector_id, pos_x, pos_y, star_type,
                 controlling_civ, infrastructure, data_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                system.id.to_string(),
                system.name,
                sector_str,
                system.position.0,
                system.position.1,
                system.star_type.to_string(),
                civ_str,
                system.infrastructure_level.to_string(),
                json,
            ],
        )?;
        Ok(())
    }

    pub fn save_civilization(&self, civ: &Civilization) -> SqlResult<()> {
        let json = serde_json::to_string(civ).expect("Civilization serialization");
        self.conn.execute(
            "INSERT OR REPLACE INTO civilizations (id, name, data_json) VALUES (?1, ?2, ?3)",
            params![civ.id.to_string(), civ.name, json],
        )?;
        Ok(())
    }

    pub fn save_faction(&self, faction: &Faction) -> SqlResult<()> {
        let json = serde_json::to_string(faction).expect("Faction serialization");
        self.conn.execute(
            "INSERT OR REPLACE INTO factions (id, name, category, data_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                faction.id.to_string(),
                faction.name,
                faction.category.to_string(),
                json,
            ],
        )?;
        Ok(())
    }

    pub fn save_connection(&self, conn: &starbound_core::galaxy::Connection) -> SqlResult<()> {
        let json = serde_json::to_string(conn).expect("Connection serialization");
        // Store with smaller UUID first for consistent ordering.
        let (a, b) = ordered_pair(conn.system_a, conn.system_b);
        self.conn.execute(
            "INSERT OR REPLACE INTO connections
                (system_a, system_b, distance_ly, route_type, data_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                a.to_string(),
                b.to_string(),
                conn.distance_ly,
                conn.route_type.to_string(),
                json,
            ],
        )?;
        Ok(())
    }

    // -------------------------------------------------------------------
    // Galaxy reads
    // -------------------------------------------------------------------

    pub fn load_all_systems(&self) -> SqlResult<Vec<StarSystem>> {
        let mut stmt = self.conn.prepare("SELECT data_json FROM star_systems")?;
        let systems = stmt
            .query_map([], |row| {
                let json: String = row.get(0)?;
                Ok(serde_json::from_str(&json).expect("StarSystem deserialization"))
            })?
            .collect::<SqlResult<Vec<_>>>()?;
        Ok(systems)
    }

    pub fn load_system(&self, id: Uuid) -> SqlResult<Option<StarSystem>> {
        let mut stmt = self
            .conn
            .prepare("SELECT data_json FROM star_systems WHERE id = ?1")?;
        let mut rows = stmt.query_map(params![id.to_string()], |row| {
            let json: String = row.get(0)?;
            Ok(serde_json::from_str(&json).expect("StarSystem deserialization"))
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn load_all_civilizations(&self) -> SqlResult<Vec<Civilization>> {
        let mut stmt = self.conn.prepare("SELECT data_json FROM civilizations")?;
        let civs = stmt
            .query_map([], |row| {
                let json: String = row.get(0)?;
                Ok(serde_json::from_str(&json).expect("Civilization deserialization"))
            })?
            .collect::<SqlResult<Vec<_>>>()?;
        Ok(civs)
    }

    pub fn load_all_factions(&self) -> SqlResult<Vec<Faction>> {
        let mut stmt = self.conn.prepare("SELECT data_json FROM factions")?;
        let factions = stmt
            .query_map([], |row| {
                let json: String = row.get(0)?;
                Ok(serde_json::from_str(&json).expect("Faction deserialization"))
            })?
            .collect::<SqlResult<Vec<_>>>()?;
        Ok(factions)
    }

    pub fn load_faction(&self, id: Uuid) -> SqlResult<Option<Faction>> {
        let mut stmt = self
            .conn
            .prepare("SELECT data_json FROM factions WHERE id = ?1")?;
        let mut rows = stmt.query_map(params![id.to_string()], |row| {
            let json: String = row.get(0)?;
            Ok(serde_json::from_str(&json).expect("Faction deserialization"))
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn load_all_connections(&self) -> SqlResult<Vec<starbound_core::galaxy::Connection>> {
        let mut stmt = self.conn.prepare("SELECT data_json FROM connections")?;
        let conns = stmt
            .query_map([], |row| {
                let json: String = row.get(0)?;
                Ok(serde_json::from_str(&json).expect("Connection deserialization"))
            })?
            .collect::<SqlResult<Vec<_>>>()?;
        Ok(conns)
    }

    pub fn load_sector(&self, id: Uuid) -> SqlResult<Option<Sector>> {
        let mut stmt = self
            .conn
            .prepare("SELECT data_json FROM sectors WHERE id = ?1")?;
        let mut rows = stmt.query_map(params![id.to_string()], |row| {
            let json: String = row.get(0)?;
            Ok(serde_json::from_str(&json).expect("Sector deserialization"))
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn load_all_sectors(&self) -> SqlResult<Vec<Sector>> {
        let mut stmt = self.conn.prepare("SELECT data_json FROM sectors")?;
        let sectors = stmt
            .query_map([], |row| {
                let json: String = row.get(0)?;
                Ok(serde_json::from_str(&json).expect("Sector deserialization"))
            })?
            .collect::<SqlResult<Vec<_>>>()?;
        Ok(sectors)
    }

    /// Find systems connected to a given system, with distances.
    pub fn load_connections_for(&self, system_id: Uuid) -> SqlResult<Vec<starbound_core::galaxy::Connection>> {
        let id_str = system_id.to_string();
        let mut stmt = self.conn.prepare(
            "SELECT data_json FROM connections WHERE system_a = ?1 OR system_b = ?1",
        )?;
        let conns = stmt
            .query_map(params![id_str], |row| {
                let json: String = row.get(0)?;
                Ok(serde_json::from_str(&json).expect("Connection deserialization"))
            })?
            .collect::<SqlResult<Vec<_>>>()?;
        Ok(conns)
    }

    // -------------------------------------------------------------------
    // Journey (player state)
    // -------------------------------------------------------------------

    pub fn save_journey(&self, journey: &Journey) -> SqlResult<()> {
        let json = serde_json::to_string(journey).expect("Journey serialization");
        self.conn.execute(
            "INSERT OR REPLACE INTO journey (id, data_json) VALUES (1, ?1)",
            params![json],
        )?;
        Ok(())
    }

    pub fn load_journey(&self) -> SqlResult<Option<Journey>> {
        let mut stmt = self
            .conn
            .prepare("SELECT data_json FROM journey WHERE id = 1")?;
        let mut rows = stmt.query_map([], |row| {
            let json: String = row.get(0)?;
            Ok(serde_json::from_str(&json).expect("Journey deserialization"))
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    // -------------------------------------------------------------------
    // Bulk save (convenience for galaxy generation)
    // -------------------------------------------------------------------

    /// Save an entire generated galaxy in a single transaction.
    pub fn save_galaxy(
        &self,
        sector: &Sector,
        systems: &[StarSystem],
        civilizations: &[Civilization],
        factions: &[Faction],
        connections: &[starbound_core::galaxy::Connection],
    ) -> SqlResult<()> {
        self.conn.execute_batch("BEGIN")?;

        self.save_sector(sector)?;
        for civ in civilizations {
            self.save_civilization(civ)?;
        }
        for faction in factions {
            self.save_faction(faction)?;
        }
        for system in systems {
            self.save_system(system, Some(sector.id))?;
        }
        for conn in connections {
            self.save_connection(conn)?;
        }

        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }
}

fn ordered_pair(a: Uuid, b: Uuid) -> (Uuid, Uuid) {
    if a.to_string() <= b.to_string() {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starbound_simulation::generate::generate_galaxy;

    #[test]
    fn round_trip_galaxy_through_sqlite() {
        let galaxy = generate_galaxy(42);
        let save = SaveFile::in_memory().expect("create in-memory db");

        // Save everything.
        save.save_galaxy(
            &galaxy.sector,
            &galaxy.systems,
            &galaxy.civilizations,
            &galaxy.factions,
            &galaxy.connections,
        )
        .expect("save galaxy");

        // Load everything back.
        let systems = save.load_all_systems().expect("load systems");
        let civs = save.load_all_civilizations().expect("load civilizations");
        let factions = save.load_all_factions().expect("load factions");
        let connections = save.load_all_connections().expect("load connections");
        let sectors = save.load_all_sectors().expect("load sectors");

        assert_eq!(systems.len(), 10);
        assert_eq!(civs.len(), 2);
        assert_eq!(factions.len(), 6);
        assert_eq!(connections.len(), galaxy.connections.len());
        assert_eq!(sectors.len(), 1);
        assert_eq!(sectors[0].name, "The Near Reach");

        // Verify a specific system round-trips correctly.
        let meridian = systems.iter().find(|s| s.name == "Meridian").expect("Meridian exists");
        assert_eq!(meridian.star_type, StarType::YellowDwarf);
        assert!(meridian.controlling_civ.is_some());

        // Verify faction_presence survived the round trip.
        assert!(
            !meridian.faction_presence.is_empty(),
            "Meridian should have faction presence after round-trip",
        );

        // Verify connections for a specific system.
        let meridian_conns = save.load_connections_for(meridian.id).expect("load connections");
        assert!(!meridian_conns.is_empty(), "Meridian should have connections");

        // Verify individual system load.
        let loaded = save.load_system(meridian.id).expect("load one").expect("exists");
        assert_eq!(loaded.name, "Meridian");
    }

    #[test]
    fn round_trip_factions_through_sqlite() {
        let galaxy = generate_galaxy(42);
        let save = SaveFile::in_memory().expect("create in-memory db");

        save.save_galaxy(
            &galaxy.sector,
            &galaxy.systems,
            &galaxy.civilizations,
            &galaxy.factions,
            &galaxy.connections,
        )
        .expect("save galaxy");

        let factions = save.load_all_factions().expect("load factions");
        assert_eq!(factions.len(), 6);

        // Verify specific faction round-trips.
        let mil_cmd = factions.iter()
            .find(|f| f.name == "Hegemony Military Command")
            .expect("Military Command exists");
        assert_eq!(mil_cmd.category, FactionCategory::Military);
        assert!(!mil_cmd.influence.is_empty());
        assert!(!mil_cmd.notable_assets.is_empty());

        // Verify individual faction load.
        let loaded = save.load_faction(mil_cmd.id).expect("load one").expect("exists");
        assert_eq!(loaded.name, "Hegemony Military Command");
        assert_eq!(loaded.category, FactionCategory::Military);
    }

    #[test]
    fn factions_and_civilizations_are_separate_tables() {
        let galaxy = generate_galaxy(42);
        let save = SaveFile::in_memory().expect("create in-memory db");

        save.save_galaxy(
            &galaxy.sector,
            &galaxy.systems,
            &galaxy.civilizations,
            &galaxy.factions,
            &galaxy.connections,
        )
        .expect("save galaxy");

        // Loading civilizations should NOT return factions and vice versa.
        let civs = save.load_all_civilizations().expect("load civs");
        let factions = save.load_all_factions().expect("load factions");

        assert_eq!(civs.len(), 2, "Should have exactly 2 civilizations");
        assert_eq!(factions.len(), 6, "Should have exactly 6 factions");

        // Names should not overlap.
        let civ_names: Vec<&str> = civs.iter().map(|c| c.name.as_str()).collect();
        let faction_names: Vec<&str> = factions.iter().map(|f| f.name.as_str()).collect();
        for name in &faction_names {
            assert!(
                !civ_names.contains(name),
                "Faction name '{}' should not appear in civilizations table",
                name,
            );
        }
    }

    #[test]
    fn faction_presence_survives_round_trip() {
        let galaxy = generate_galaxy(42);
        let save = SaveFile::in_memory().expect("create in-memory db");

        save.save_galaxy(
            &galaxy.sector,
            &galaxy.systems,
            &galaxy.civilizations,
            &galaxy.factions,
            &galaxy.connections,
        )
        .expect("save galaxy");

        let systems = save.load_all_systems().expect("load systems");

        // Every system should preserve its faction_presence.
        for original in &galaxy.systems {
            let loaded = systems.iter().find(|s| s.name == original.name)
                .unwrap_or_else(|| panic!("System {} missing after round-trip", original.name));
            assert_eq!(
                original.faction_presence.len(),
                loaded.faction_presence.len(),
                "System {} faction_presence count changed after round-trip",
                original.name,
            );
        }
    }

    #[test]
    fn round_trip_journey_through_sqlite() {
        use std::collections::HashMap;
        use starbound_core::crew::*;
        use starbound_core::mission::*;
        use starbound_core::ship::*;
        use starbound_core::time::Timestamp;

        let galaxy = generate_galaxy(42);
        let save = SaveFile::in_memory().expect("create in-memory db");

        save.save_galaxy(
            &galaxy.sector,
            &galaxy.systems,
            &galaxy.civilizations,
            &galaxy.factions,
            &galaxy.connections,
        )
        .expect("save galaxy");

        let start_system = galaxy.systems[0].id;

        let journey = Journey {
            ship: Ship {
                name: "The Quiet Reach".into(),
                hull_condition: 0.9,
                fuel: 80.0,
                fuel_capacity: 100.0,
                cargo: HashMap::new(),
                cargo_capacity: 50,
                modules: ShipModules {
                    engine: Module::standard("Kessler-IV Sublight Drive"),
                    sensors: Module::standard("Broadband Array"),
                    comms: Module::standard("Standard Ansible Relay"),
                    weapons: Module::standard("Point Defense Grid"),
                    life_support: Module::standard("Closed-Loop Atmospheric"),
                },
            },
            current_system: start_system,
            time: Timestamp::zero(),
            resources: 5000.0,
            mission: MissionState {
                mission_type: MissionType::Search,
                core_truth: "The signal is a warning.".into(),
                knowledge_nodes: vec![],
            },
            crew: vec![CrewMember {
                id: Uuid::new_v4(),
                name: "Lena Vasquez".into(),
                role: CrewRole::Navigator,
                drives: PersonalityDrives {
                    security: 0.3,
                    freedom: 0.5,
                    purpose: 0.7,
                    connection: 0.6,
                    knowledge: 0.8,
                    justice: 0.4,
                },
                trust: Trust::starting_crew(),
                relationships: HashMap::new(),
                background: "Former cartographer.".into(),
                state: CrewState {
                    mood: Mood::Determined,
                    stress: 0.2,
                    active_concerns: vec![],
                },
                origin: CrewOrigin::Starting,
            }],
            threads: vec![],
            event_log: vec![],
            civ_standings: HashMap::new(),
        };

        save.save_journey(&journey).expect("save journey");

        let loaded = save.load_journey().expect("load").expect("exists");
        assert_eq!(loaded.ship.name, "The Quiet Reach");
        assert_eq!(loaded.current_system, start_system);
        assert_eq!(loaded.crew.len(), 1);
        assert_eq!(loaded.crew[0].name, "Lena Vasquez");
    }

    #[test]
    fn save_file_on_disk() {
        let galaxy = generate_galaxy(99);
        let path = std::env::temp_dir().join("starbound_test.db");

        // Clean up from any previous run.
        let _ = std::fs::remove_file(&path);

        {
            let save = SaveFile::create(&path).expect("create file");
            save.save_galaxy(
                &galaxy.sector,
                &galaxy.systems,
                &galaxy.civilizations,
                &galaxy.factions,
                &galaxy.connections,
            )
            .expect("save");
        }

        // Reopen and verify.
        {
            let save = SaveFile::open(&path).expect("reopen");
            let systems = save.load_all_systems().expect("load");
            assert_eq!(systems.len(), 10);

            let factions = save.load_all_factions().expect("load factions");
            assert_eq!(factions.len(), 6);
        }

        // Clean up.
        let _ = std::fs::remove_file(&path);
    }
}