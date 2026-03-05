// file: crates/simulation/src/generate.rs
//! Galaxy generation — deterministic from a seed.
//!
//! One sector, ten systems, 2–5 civilizations (template-driven),
//! 5–8 factions (template-driven). Expansion comes later.

use rand::prelude::*;
use std::collections::HashMap;
use uuid::Uuid;

use starbound_core::galaxy::*;
use starbound_core::npc::Npc;
use starbound_core::time::Timestamp;

use crate::templates;

/// The output of galaxy generation — everything needed to start a game.
pub struct GeneratedGalaxy {
    pub sector: Sector,
    pub systems: Vec<StarSystem>,
    pub civilizations: Vec<Civilization>,
    pub factions: Vec<Faction>,
    pub connections: Vec<Connection>,
    pub npcs: Vec<Npc>,
    /// The system where new games begin — a contested hub between civilizations.
    pub start_system_id: Uuid,
}

pub fn generate_galaxy(seed: u64) -> GeneratedGalaxy {
    let mut rng = StdRng::seed_from_u64(seed);

    let mut civilizations = generate_civilizations(&mut rng);
    let mut systems = generate_systems(&mut rng, &civilizations);

    // Identify the start system — the first Hub-level system, or first system as fallback.
    let start_system_id = systems.iter()
        .find(|s| s.infrastructure_level == InfrastructureLevel::Hub && s.controlling_civ.is_none())
        .or_else(|| systems.iter().find(|s| s.infrastructure_level == InfrastructureLevel::Hub))
        .unwrap_or(&systems[0])
        .id;

    let connections = generate_connections(&systems, &mut rng);

    let factions = generate_factions(&mut rng, &civilizations);

    // Wire faction IDs into their parent civilizations.
    wire_factions_into_civs(&mut civilizations, &factions);

    // Wire source_faction into existing CivPressures where appropriate.
    wire_pressure_sources(&mut civilizations, &factions);

    // Populate faction_presence on every system.
    assign_faction_presence(&mut systems, &factions, &civilizations);

    // Generate economy profiles for inhabited systems.
    assign_system_economies(&mut systems, &factions, &mut rng);

    let num_civs = civilizations.len();
    let sector_desc = match num_civs {
        2 => "The first settled systems beyond the homeworld. \
              Old colonies, older grudges. Two powers and a lot of \
              empty space between them.",
        3 => "The first settled systems beyond the homeworld. \
              Three civilizations share these stars — uneasily. \
              Contested borders and frontier space that answers to no one.",
        _ => "The first settled systems beyond the homeworld. \
              Multiple civilizations, contested borders, and more \
              empty space than anyone can claim.",
    };

    let sector = Sector {
        id: Uuid::new_v4(),
        name: "The Near Reach".into(),
        description: sector_desc.into(),
        system_ids: systems.iter().map(|s| s.id).collect(),
    };

    let npcs = generate_npcs(&systems, &factions, &mut rng);

    GeneratedGalaxy {
        sector,
        systems,
        civilizations,
        factions,
        connections,
        npcs,
        start_system_id,
    }
}

// ===========================================================================
// Civilization generation (template-driven)
// ===========================================================================

/// Generate civilizations from `data/templates/civilizations.json`.
///
/// Produces `default_count` civilizations (typically 3) with:
/// - Procedural names assembled from prefix + suffix pools
/// - Ethos derived from suffix weights + prefix bias + noise
/// - Capabilities derived from government style + ethos
/// - Inter-civ relationships based on ethos compatibility
fn generate_civilizations(rng: &mut StdRng) -> Vec<Civilization> {
    let t = templates::load_civ_templates();
    let civ_count = t.generation_rules.default_count;

    let mut used_suffixes: Vec<String> = Vec::new();
    let mut civs: Vec<Civilization> = Vec::with_capacity(civ_count);

    for _ in 0..civ_count {
        let suffix_idx = pick_civ_suffix(rng, &t, &used_suffixes);
        let suffix = &t.suffixes[suffix_idx];
        used_suffixes.push(suffix.name.clone());

        let prefix_idx = pick_civ_prefix(rng, &t, &suffix.name);
        let prefix = &t.prefixes[prefix_idx];

        let ethos = compute_civ_ethos(rng, suffix, prefix, t.generation_rules.ethos_noise);
        let capabilities = derive_capabilities(&suffix.government_style, &ethos, rng);
        let name = assemble_civ_name(&prefix.name, &suffix.name);

        let stability = rng.gen_range(
            t.initial_state_ranges.stability[0]..=t.initial_state_ranges.stability[1],
        );

        civs.push(Civilization {
            id: Uuid::new_v4(),
            name,
            ethos,
            capabilities,
            relationships: HashMap::new(),
            internal_dynamics: InternalDynamics {
                stability,
                pressures: generate_pressures(rng, &ethos),
            },
            faction_ids: vec![],
        });
    }

    // Wire inter-civ relationships.
    wire_civ_relationships(&mut civs, rng);

    // Shuffle order so downstream generation isn't biased by creation order.
    civs.shuffle(rng);

    civs
}

// ---------------------------------------------------------------------------
// Name assembly helpers
// ---------------------------------------------------------------------------

/// Pick a suffix index that hasn't been used yet, weighted by template weight.
fn pick_civ_suffix(
    rng: &mut StdRng,
    templates: &templates::CivTemplates,
    used: &[String],
) -> usize {
    let available: Vec<(usize, f64)> = templates
        .suffixes
        .iter()
        .enumerate()
        .filter(|(_, s)| !used.contains(&s.name))
        .map(|(i, s)| (i, s.weight))
        .collect();
    assert!(!available.is_empty(), "Ran out of suffixes — need more than civs");
    let local_idx = pick_weighted(rng, &available, |item| item.1);
    available[local_idx].0
}

/// Pick a prefix index compatible with the chosen suffix, weighted by template weight.
fn pick_civ_prefix(
    rng: &mut StdRng,
    templates: &templates::CivTemplates,
    suffix_name: &str,
) -> usize {
    let available: Vec<(usize, f64)> = templates
        .prefixes
        .iter()
        .enumerate()
        .filter(|(_, p)| !templates.compatibility.is_blocked(&p.name, suffix_name))
        .map(|(i, p)| (i, p.weight))
        .collect();
    assert!(
        !available.is_empty(),
        "No compatible prefixes for suffix '{}'",
        suffix_name,
    );
    let local_idx = pick_weighted(rng, &available, |item| item.1);
    available[local_idx].0
}

/// Weighted random selection. Returns index into the slice.
fn pick_weighted<T>(rng: &mut StdRng, items: &[T], weight_fn: impl Fn(&T) -> f64) -> usize {
    let total: f64 = items.iter().map(&weight_fn).sum();
    let mut roll = rng.gen_range(0.0..total);
    for (i, item) in items.iter().enumerate() {
        roll -= weight_fn(item);
        if roll <= 0.0 {
            return i;
        }
    }
    items.len() - 1 // Floating-point safety net.
}

/// Assemble a civilization name from prefix + suffix.
///
/// Some suffixes read more naturally with "The" (Compact, Collective,
/// Assembly). Others stand on their own (Hegemony, Federation, Dominion).
fn assemble_civ_name(prefix: &str, suffix: &str) -> String {
    let needs_the = matches!(
        suffix,
        "Compact" | "Collective" | "Assembly" | "Ascendancy"
    );
    if needs_the {
        format!("The {} {}", prefix, suffix)
    } else {
        format!("{} {}", prefix, suffix)
    }
}

/// Extract the cultural prefix from a civ name (for faction naming).
/// "Terran Hegemony" → "Terran", "The Solari Collective" → "Solari".
pub fn extract_civ_prefix(civ_name: &str) -> &str {
    let name = civ_name.strip_prefix("The ").unwrap_or(civ_name);
    name.split_whitespace().next().unwrap_or(name)
}

// ---------------------------------------------------------------------------
// Ethos & capabilities
// ---------------------------------------------------------------------------

/// Compute civilization ethos from suffix base weights + prefix bias + noise.
fn compute_civ_ethos(
    rng: &mut StdRng,
    suffix: &templates::CivSuffix,
    prefix: &templates::CivPrefix,
    noise: f32,
) -> CivEthos {
    let mut get = |axis: &str| -> f32 {
        let base = suffix.ethos_weights.get(axis).copied().unwrap_or(0.0);
        let bias = prefix.ethos_bias.get(axis).copied().unwrap_or(0.0);
        let jitter: f32 = rng.gen_range(-noise..=noise);
        (base + bias + jitter).clamp(0.0, 1.0)
    };

    CivEthos {
        expansionist: get("expansionist"),
        isolationist: get("isolationist"),
        militaristic: get("militaristic"),
        diplomatic: get("diplomatic"),
        theocratic: get("theocratic"),
        mercantile: get("mercantile"),
        technocratic: get("technocratic"),
        communal: get("communal"),
    }
}

/// Derive capabilities from government style, modulated by ethos.
fn derive_capabilities(
    government_style: &str,
    ethos: &CivEthos,
    rng: &mut StdRng,
) -> CivCapabilities {
    // Base ranges by government style: (size, wealth, technology, military)
    let (size_r, wealth_r, tech_r, mil_r) = match government_style {
        "autocratic"   => ((0.55, 0.80), (0.45, 0.70), (0.35, 0.60), (0.55, 0.80)),
        "confederal"   => ((0.30, 0.55), (0.45, 0.70), (0.40, 0.60), (0.20, 0.45)),
        "federal"      => ((0.40, 0.65), (0.40, 0.65), (0.40, 0.65), (0.35, 0.60)),
        "collective"   => ((0.30, 0.50), (0.40, 0.60), (0.50, 0.75), (0.15, 0.40)),
        "democratic"   => ((0.40, 0.60), (0.40, 0.65), (0.40, 0.70), (0.25, 0.50)),
        "oligarchic"   => ((0.40, 0.60), (0.55, 0.80), (0.40, 0.65), (0.30, 0.55)),
        "theocratic"   => ((0.30, 0.55), (0.30, 0.55), (0.20, 0.45), (0.30, 0.55)),
        "meritocratic" => ((0.35, 0.60), (0.45, 0.70), (0.55, 0.80), (0.25, 0.50)),
        _              => ((0.35, 0.60), (0.40, 0.60), (0.40, 0.60), (0.30, 0.55)),
    };

    let mut range_val = |r: (f32, f32), ethos_mod: f32| -> f32 {
        let base: f32 = rng.gen_range(r.0..=r.1);
        (base + ethos_mod * 0.15).clamp(0.1, 0.95)
    };

    CivCapabilities {
        size: range_val(size_r, ethos.expansionist - ethos.isolationist),
        wealth: range_val(wealth_r, ethos.mercantile),
        technology: range_val(tech_r, ethos.technocratic),
        military: range_val(mil_r, ethos.militaristic),
    }
}

// ---------------------------------------------------------------------------
// Internal pressures
// ---------------------------------------------------------------------------

/// Generate 1–3 internal pressures based on ethos tensions.
fn generate_pressures(rng: &mut StdRng, ethos: &CivEthos) -> Vec<CivPressure> {
    let pool: Vec<(bool, &str)> = vec![
        (ethos.militaristic > 0.5,
         "Military factions push for increased defense spending along the frontier"),
        (ethos.expansionist > 0.5,
         "Expansionist elements lobby for new colony charters in unclaimed space"),
        (ethos.isolationist > 0.4,
         "Border-closure advocates gain popular support in the inner systems"),
        (ethos.mercantile > 0.5,
         "Trade guilds pressure the government for reduced tariffs"),
        (ethos.theocratic > 0.3,
         "Religious authorities demand greater cultural oversight"),
        (ethos.communal > 0.5 && ethos.mercantile > 0.3,
         "Tension between communal ideals and growing commercial interests"),
        (ethos.militaristic > 0.4 && ethos.diplomatic > 0.4,
         "Hawks and diplomats clash over foreign policy direction"),
        (ethos.technocratic > 0.5,
         "Technocratic elite face populist pushback from outer colonies"),
        (ethos.expansionist < 0.3 && ethos.isolationist < 0.3,
         "Stagnation concerns — neither expanding nor consolidating"),
        (ethos.communal < 0.3,
         "Outer colony autonomy movements gaining support"),
    ];

    let mut eligible: Vec<&str> = pool
        .iter()
        .filter(|(cond, _)| *cond)
        .map(|(_, desc)| *desc)
        .collect();

    if eligible.is_empty() {
        eligible.push("Shifting demographics reshape the political landscape");
    }

    let count = rng.gen_range(1..=eligible.len().min(3));
    eligible.shuffle(rng);

    eligible[..count]
        .iter()
        .map(|desc| CivPressure {
            description: desc.to_string(),
            source_faction: None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Inter-civ relationships
// ---------------------------------------------------------------------------

/// Generate relationships between all pairs of civilizations based on
/// ethos compatibility. Similar values → warmer relations; opposing → colder.
fn wire_civ_relationships(civs: &mut [Civilization], rng: &mut StdRng) {
    let ids: Vec<Uuid> = civs.iter().map(|c| c.id).collect();

    let ethos_vecs: Vec<[f32; 8]> = civs
        .iter()
        .map(|c| [
            c.ethos.expansionist,
            c.ethos.isolationist,
            c.ethos.militaristic,
            c.ethos.diplomatic,
            c.ethos.theocratic,
            c.ethos.mercantile,
            c.ethos.technocratic,
            c.ethos.communal,
        ])
        .collect();

    for i in 0..civs.len() {
        for j in 0..civs.len() {
            if i == j {
                continue;
            }

            let similarity: f32 = ethos_vecs[i]
                .iter()
                .zip(ethos_vecs[j].iter())
                .map(|(a, b)| 1.0 - (a - b).abs())
                .sum::<f32>()
                / 8.0;

            let diplomatic_base = (similarity - 0.5) * 1.0;
            let jitter: f32 = rng.gen_range(-0.15..=0.15);
            let diplomatic = (diplomatic_base + jitter).clamp(-1.0, 1.0);

            let econ_base = (ethos_vecs[i][5] + ethos_vecs[j][5]) / 2.0;
            let economic = (econ_base * 0.6 + rng.gen_range(0.0..0.2_f32)).clamp(0.0, 1.0);

            let mil_threat = ethos_vecs[i][2] * ethos_vecs[j][2];
            let military = (diplomatic * 0.5 - mil_threat * 0.3
                + rng.gen_range(-0.1..0.1_f32))
                .clamp(-1.0, 1.0);

            civs[i].relationships.insert(
                ids[j],
                CivDisposition {
                    diplomatic,
                    economic,
                    military,
                },
            );
        }
    }
}

// ===========================================================================
// Faction generation (template-driven)
// ===========================================================================

fn generate_factions(rng: &mut StdRng, civs: &[Civilization]) -> Vec<Faction> {
    let t = templates::load_faction_templates();
    let all_civ_ids: Vec<Uuid> = civs.iter().map(|c| c.id).collect();

    let mut factions: Vec<Faction> = Vec::new();
    let mut mil_civ_idx = 0_usize; // Track which civ gets next military faction.

    // Generate guaranteed factions.
    for category_name in &t.generation_rules.guaranteed {
        let cat = t.categories.get(category_name.as_str())
            .unwrap_or_else(|| panic!("Missing guaranteed category '{}'", category_name));

        let faction = build_faction_from_template(
            rng, category_name, cat, civs, &all_civ_ids, &mut mil_civ_idx,
        );
        factions.push(faction);
    }

    // Roll optional factions.
    for rule in &t.generation_rules.optional {
        if factions.len() >= t.generation_rules.max_factions {
            break;
        }
        let roll: f64 = rng.gen();
        if roll < rule.chance {
            if let Some(cat) = t.categories.get(rule.category.as_str()) {
                let faction = build_faction_from_template(
                    rng, &rule.category, cat, civs, &all_civ_ids, &mut mil_civ_idx,
                );
                factions.push(faction);
            }
        }
    }

    factions
}

/// Build a single faction from a category template.
fn build_faction_from_template(
    rng: &mut StdRng,
    category_name: &str,
    cat: &templates::FactionCategoryTemplate,
    civs: &[Civilization],
    all_civ_ids: &[Uuid],
    mil_civ_idx: &mut usize,
) -> Faction {
    // Determine parent civ for civ_internal factions.
    let parent_civ_idx = if cat.scope == "civ_internal" {
        let idx = (*mil_civ_idx).min(civs.len() - 1);
        *mil_civ_idx += 1;
        Some(idx)
    } else {
        None
    };

    let parent_civ = parent_civ_idx.map(|i| &civs[i]);
    let civ_prefix = parent_civ
        .map(|c| extract_civ_prefix(&c.name).to_owned())
        .unwrap_or_default();

    // Pick name pattern and fill slots.
    let name = generate_faction_name(rng, cat, &civ_prefix);

    // Compute FactionCategory enum from the template category string.
    let fc = match category_name {
        "military" => FactionCategory::Military,
        "economic" => FactionCategory::Economic,
        "guild" => FactionCategory::Guild,
        "religious" => FactionCategory::Religious,
        "criminal_frontier" | "criminal_covert" => FactionCategory::Criminal,
        _ => FactionCategory::Guild, // Fallback.
    };

    // Build scope.
    let scope = match cat.scope.as_str() {
        "civ_internal" => FactionScope::CivInternal {
            civ_id: parent_civ.unwrap().id,
        },
        "transnational" => FactionScope::Transnational {
            civ_ids: all_civ_ids.to_vec(),
        },
        _ => FactionScope::Independent,
    };

    // Compute ethos: base_traits + component biases already folded into name selection.
    // Template secrecy → maps to 1.0 - openness.
    let bt = &cat.base_traits;
    let noise = 0.1_f32;
    let ethos = FactionEthos {
        alignment: (bt.alignment + rng.gen_range(-noise..=noise)).clamp(-1.0, 1.0),
        openness: ((1.0 - bt.secrecy) * 0.5 + bt.openness * 0.5
            + rng.gen_range(-noise..=noise))
            .clamp(0.0, 1.0),
        aggression: (bt.aggression + rng.gen_range(-noise..=noise)).clamp(0.0, 1.0),
    };

    // Build influence map.
    let influence = build_faction_influence(rng, &scope, civs, category_name);

    // Generate description and assets from category.
    let description = generate_faction_description(
        category_name, &name, parent_civ, &civ_prefix,
    );
    let notable_assets = generate_faction_assets(
        category_name, &civ_prefix,
    );

    // Convert template services to enum.
    let _services: Vec<FactionService> = cat
        .default_services
        .iter()
        .filter_map(|s| parse_faction_service(s))
        .collect();

    Faction {
        id: Uuid::new_v4(),
        name,
        category: fc,
        scope,
        ethos,
        influence,
        player_standing: FactionStanding::unknown(),
        description,
        notable_assets,
    }
}

/// Fill a name pattern's {slot} placeholders from component pools.
fn generate_faction_name(
    rng: &mut StdRng,
    cat: &templates::FactionCategoryTemplate,
    civ_prefix: &str,
) -> String {
    let pattern_idx = rng.gen_range(0..cat.name_patterns.len());
    let pattern = &cat.name_patterns[pattern_idx];

    let mut result = pattern.clone();

    // Replace {civ_prefix} with the parent civ's cultural prefix.
    result = result.replace("{civ_prefix}", civ_prefix);

    // Replace all other {slot} references from component pools.
    // Iterate until no more {slots} remain (handles patterns with multiple slots).
    loop {
        let start = match result.find('{') {
            Some(i) => i,
            None => break,
        };
        let end = match result[start..].find('}') {
            Some(i) => start + i,
            None => break,
        };
        let slot_name = &result[start + 1..end];

        if let Some(components) = cat.components.get(slot_name) {
            let idx = pick_weighted(rng, components, |c| c.weight);
            let component_name = &components[idx].name;
            result = format!("{}{}{}", &result[..start], component_name, &result[end + 1..]);
        } else {
            // Unknown slot — leave as-is and break to avoid infinite loop.
            break;
        }
    }

    result
}

/// Build influence map based on faction scope and category.
fn build_faction_influence(
    rng: &mut StdRng,
    scope: &FactionScope,
    civs: &[Civilization],
    category_name: &str,
) -> HashMap<Uuid, f32> {
    let mut influence = HashMap::new();

    match scope {
        FactionScope::CivInternal { civ_id } => {
            // Strong influence in parent civ.
            influence.insert(*civ_id, rng.gen_range(0.5..0.8));
        }
        FactionScope::Transnational { civ_ids } => {
            for civ_id in civ_ids {
                let base = match category_name {
                    "economic" => rng.gen_range(0.3..0.6),
                    "criminal_covert" => rng.gen_range(0.1..0.3),
                    _ => rng.gen_range(0.2..0.4),
                };
                influence.insert(*civ_id, base);
            }
        }
        FactionScope::Independent => {
            // Independent factions have weak diffuse influence.
            for civ in civs {
                influence.insert(civ.id, rng.gen_range(0.05..0.2));
            }
        }
    }

    influence
}

/// Generate a thematic description for a faction based on its category.
fn generate_faction_description(
    category_name: &str,
    faction_name: &str,
    parent_civ: Option<&Civilization>,
    _civ_prefix: &str,
) -> String {
    match category_name {
        "military" => {
            let civ_name = parent_civ.map(|c| c.name.as_str()).unwrap_or("its parent civilization");
            format!(
                "The enforcement and defense arm of {}. {} runs border patrols, \
                 military installations, and classified research programs. Known for \
                 thoroughness, institutional loyalty, and a tendency to classify everything.",
                civ_name, faction_name,
            )
        }
        "economic" => format!(
            "A powerful merchant network operating across the Near Reach. {} manages \
             trade posts, negotiates tariffs, and maintains the commercial infrastructure \
             that keeps the civilizations fed and supplied. Officially neutral in politics; \
             practically, they lean toward whoever offers better terms.",
            faction_name,
        ),
        "guild" => format!(
            "A loose professional union of pilots, engineers, and independent spacers. \
             {} has no headquarters, no hierarchy worth mentioning — just a network of \
             mutual aid and shared expertise. The kind of organization that exists because \
             space is hard and nobody else will help when your life support fails.",
            faction_name,
        ),
        "religious" => format!(
            "A contemplative order that believes time distortion is evidence of something \
             greater — a pattern in the fabric of spacetime that rewards careful observation. \
             {} maintains monasteries in systems with high time factors. Quiet, patient, \
             occasionally unsettling.",
            faction_name,
        ),
        "criminal_frontier" => format!(
            "Frontier salvage outfit that picks over derelicts, abandoned stations, and \
             anything the civilizations left behind. {} operates where authority is thin. \
             The line between salvage and piracy is a legal distinction they don't \
             spend much time worrying about.",
            faction_name,
        ),
        "criminal_covert" => format!(
            "An information broker network that sells intelligence to anyone who can pay. \
             Nobody knows who runs {}. What everyone knows is that if you need to find \
             something out — a shipping manifest, a classified patrol route, a person \
             who doesn't want to be found — they can probably help. For a price.",
            faction_name,
        ),
        _ => format!("{} operates in the Near Reach.", faction_name),
    }
}

/// Generate notable assets for a faction based on its category.
fn generate_faction_assets(category_name: &str, civ_prefix: &str) -> Vec<String> {
    match category_name {
        "military" => vec![
            format!("Naval yards at the {} capital", civ_prefix),
            "Classified deep-space listening posts".into(),
            "Agent network in contested systems".into(),
        ],
        "economic" => vec![
            "Trade posts at major hubs".into(),
            "Interstellar cargo fleet".into(),
            "Trade route maps and tariff agreements".into(),
        ],
        "guild" => vec![
            "Repair yards at major ports".into(),
            "Informal route intelligence network".into(),
            "Emergency beacon response protocol".into(),
        ],
        "religious" => vec![
            "Monastery in distorted space".into(),
            "Extensive records of time-distortion phenomena".into(),
            "Meditation techniques that mitigate temporal disorientation".into(),
        ],
        "criminal_frontier" => vec![
            "Hidden depot in frontier space".into(),
            "Salvage fleet — three modified haulers".into(),
            "Black market contacts across the frontier".into(),
        ],
        "criminal_covert" => vec![
            "Dead drop network across the Near Reach".into(),
            "Encrypted communications infrastructure".into(),
            "Dossiers on key figures in all civilizations".into(),
        ],
        _ => vec![],
    }
}

/// Parse a template service string into a FactionService enum.
fn parse_faction_service(s: &str) -> Option<FactionService> {
    match s {
        "missions" => Some(FactionService::Missions),
        "trade" => Some(FactionService::Trade),
        "intelligence" => Some(FactionService::Intelligence),
        "repair" => Some(FactionService::Repair),
        "smuggling" => Some(FactionService::Smuggling),
        "training" => Some(FactionService::Training),
        "shelter" => Some(FactionService::Shelter),
        _ => None,
    }
}

// ===========================================================================
// Wiring factions into civilizations
// ===========================================================================

/// Push faction IDs into each civilization's `faction_ids` list.
fn wire_factions_into_civs(civs: &mut [Civilization], factions: &[Faction]) {
    for faction in factions {
        match &faction.scope {
            FactionScope::CivInternal { civ_id } => {
                if let Some(civ) = civs.iter_mut().find(|c| c.id == *civ_id) {
                    civ.faction_ids.push(faction.id);
                }
            }
            FactionScope::Transnational { civ_ids } => {
                for civ_id in civ_ids {
                    if let Some(civ) = civs.iter_mut().find(|c| c.id == *civ_id) {
                        civ.faction_ids.push(faction.id);
                    }
                }
            }
            FactionScope::Independent => {}
        }
    }
}

/// Link CivPressure entries to corresponding factions by category.
fn wire_pressure_sources(civs: &mut [Civilization], factions: &[Faction]) {
    let mil_id = factions
        .iter()
        .find(|f| f.category == FactionCategory::Military)
        .map(|f| f.id);

    let econ_id = factions
        .iter()
        .find(|f| f.category == FactionCategory::Economic)
        .map(|f| f.id);

    let religious_id = factions
        .iter()
        .find(|f| f.category == FactionCategory::Religious)
        .map(|f| f.id);

    for civ in civs.iter_mut() {
        for pressure in &mut civ.internal_dynamics.pressures {
            let desc_lower = pressure.description.to_lowercase();
            if desc_lower.contains("military") || desc_lower.contains("defense")
                || desc_lower.contains("hawks")
            {
                pressure.source_faction = mil_id;
            } else if desc_lower.contains("trade") || desc_lower.contains("tariff")
                || desc_lower.contains("commercial")
            {
                pressure.source_faction = econ_id;
            } else if desc_lower.contains("religious")
                || desc_lower.contains("cultural oversight")
            {
                pressure.source_faction = religious_id;
            }
        }
    }
}

// ===========================================================================
// Faction presence on systems (rule-based by category + system properties)
// ===========================================================================

fn assign_faction_presence(
    systems: &mut [StarSystem],
    factions: &[Faction],
    _civs: &[Civilization],
) {
    // Build lookup tables by category/scope for efficient assignment.
    let military_factions: Vec<&Faction> = factions
        .iter()
        .filter(|f| f.category == FactionCategory::Military)
        .collect();

    let economic_factions: Vec<&Faction> = factions
        .iter()
        .filter(|f| f.category == FactionCategory::Economic)
        .collect();

    let guild_factions: Vec<&Faction> = factions
        .iter()
        .filter(|f| f.category == FactionCategory::Guild)
        .collect();

    let religious_factions: Vec<&Faction> = factions
        .iter()
        .filter(|f| f.category == FactionCategory::Religious)
        .collect();

    let criminal_frontier: Vec<&Faction> = factions
        .iter()
        .filter(|f| {
            f.category == FactionCategory::Criminal
                && matches!(f.scope, FactionScope::Independent)
        })
        .collect();

    let criminal_covert: Vec<&Faction> = factions
        .iter()
        .filter(|f| {
            f.category == FactionCategory::Criminal
                && matches!(f.scope, FactionScope::Transnational { .. })
        })
        .collect();

    for system in systems.iter_mut() {
        let mut presence = Vec::new();
        let infra = system.infrastructure_level;
        let has_civ = system.controlling_civ.is_some();
        let is_distorted = system.time_factor > 1.0;

        // ----- Military factions -----
        // Strong in parent civ's capital/colonies, moderate in other civ systems.
        for mil in &military_factions {
            let parent_civ_id = match &mil.scope {
                FactionScope::CivInternal { civ_id } => Some(*civ_id),
                _ => None,
            };

            let is_parent_system = parent_civ_id
                .map_or(false, |id| system.controlling_civ == Some(id));

            let (strength, visibility) = if is_parent_system {
                match infra {
                    InfrastructureLevel::Capital => (0.9, 1.0),
                    InfrastructureLevel::Colony | InfrastructureLevel::Established => (0.5, 0.8),
                    InfrastructureLevel::Hub => (0.4, 0.6),
                    _ => (0.2, 0.3),
                }
            } else if has_civ {
                // Presence in other civ's territory — intelligence ops.
                match infra {
                    InfrastructureLevel::Capital | InfrastructureLevel::Hub => (0.15, 0.05),
                    InfrastructureLevel::Established | InfrastructureLevel::Colony
                    | InfrastructureLevel::Outpost => (0.1, 0.05),
                    _ => continue,
                }
            } else {
                // Frontier — minimal presence.
                match infra {
                    InfrastructureLevel::Outpost => (0.1, 0.05),
                    _ => continue,
                }
            };

            let services = if is_parent_system && infra >= InfrastructureLevel::Colony {
                vec![FactionService::Missions, FactionService::Intelligence, FactionService::Repair]
            } else {
                vec![FactionService::Intelligence]
            };

            presence.push(FactionPresence {
                faction_id: mil.id,
                strength,
                visibility,
                services,
            });
        }

        // ----- Economic factions -----
        // Present at hubs and capitals of all operating civs.
        for econ in &economic_factions {
            let (strength, visibility) = match infra {
                InfrastructureLevel::Hub => (0.8, 1.0),
                InfrastructureLevel::Capital => (0.5, 0.8),
                InfrastructureLevel::Established => (0.4, 0.7),
                InfrastructureLevel::Colony => (0.3, 0.6),
                InfrastructureLevel::Outpost if has_civ => (0.15, 0.4),
                _ => continue,
            };

            let services = if infra >= InfrastructureLevel::Established {
                vec![FactionService::Trade, FactionService::Missions, FactionService::Repair]
            } else {
                vec![FactionService::Trade]
            };

            presence.push(FactionPresence {
                faction_id: econ.id,
                strength,
                visibility,
                services,
            });
        }

        // ----- Guild factions -----
        // Scattered at established+ infrastructure systems.
        for guild in &guild_factions {
            let (strength, visibility) = match infra {
                InfrastructureLevel::Capital | InfrastructureLevel::Hub => (0.4, 0.7),
                InfrastructureLevel::Established => (0.3, 0.6),
                InfrastructureLevel::Colony => (0.25, 0.5),
                InfrastructureLevel::Outpost => (0.15, 0.4),
                InfrastructureLevel::None => continue,
            };

            let services = if infra >= InfrastructureLevel::Established {
                vec![FactionService::Repair, FactionService::Trade, FactionService::Training]
            } else {
                vec![FactionService::Repair]
            };

            presence.push(FactionPresence {
                faction_id: guild.id,
                strength,
                visibility,
                services,
            });
        }

        // ----- Religious factions -----
        // Drawn to distorted space (time_factor > 1.0). Absent from normal systems.
        for rel in &religious_factions {
            if !is_distorted {
                continue;
            }
            let distortion_pull = (system.time_factor.log2() / 5.0).min(1.0) as f32;
            let (strength, visibility) = (
                0.3 + distortion_pull * 0.4,
                0.4 + distortion_pull * 0.3,
            );

            let services = if is_distorted {
                vec![FactionService::Shelter, FactionService::Training, FactionService::Intelligence]
            } else {
                vec![FactionService::Shelter]
            };

            presence.push(FactionPresence {
                faction_id: rel.id,
                strength,
                visibility,
                services,
            });
        }

        // ----- Criminal (frontier) -----
        // Outposts and unclaimed systems. Also weak at low-infra civ systems.
        for crim in &criminal_frontier {
            let (strength, visibility) = if !has_civ {
                match infra {
                    InfrastructureLevel::Outpost => (0.7, 0.4),
                    InfrastructureLevel::None => (0.4, 0.3),
                    _ => (0.5, 0.35),
                }
            } else {
                match infra {
                    InfrastructureLevel::Outpost | InfrastructureLevel::Colony => (0.2, 0.15),
                    _ => continue,
                }
            };

            let services = vec![
                FactionService::Trade, FactionService::Repair,
                FactionService::Smuggling, FactionService::Shelter,
            ];

            presence.push(FactionPresence {
                faction_id: crim.id,
                strength,
                visibility,
                services,
            });
        }

        // ----- Criminal (covert) -----
        // Low-visibility presence in capitals and hubs.
        for covert in &criminal_covert {
            let (strength, visibility) = match infra {
                InfrastructureLevel::Capital => (0.3, 0.1),
                InfrastructureLevel::Hub => (0.4, 0.1),
                InfrastructureLevel::Established => (0.2, 0.05),
                _ => continue,
            };

            presence.push(FactionPresence {
                faction_id: covert.id,
                strength,
                visibility,
                services: vec![FactionService::Intelligence, FactionService::Smuggling],
            });
        }

        system.faction_presence = presence;
    }
}

// ===========================================================================
// System generation (template-driven)
// ===========================================================================

/// Role assigned during generation — determines infrastructure and civ ownership.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SystemRole {
    /// A civilization's seat of power.
    Capital(usize),
    /// Core territory of a civilization.
    Core(usize),
    /// Outer territory of a civilization.
    Colony(usize),
    /// A contested trade hub between civilizations (start system).
    Hub,
    /// Frontier with minimal civilization.
    Frontier,
    /// Uninhabited wilderness.
    Wilderness,
}

fn generate_systems(rng: &mut StdRng, civs: &[Civilization]) -> Vec<StarSystem> {
    let t = templates::load_system_templates();
    let num_civs = civs.len();
    let civ_ids: Vec<Uuid> = civs.iter().map(|c| c.id).collect();
    let system_count = t.generation_rules.system_count;
    let spread = t.generation_rules.spatial_spread;

    // --- Assign roles ---
    // N capitals + 1 hub + territory + frontier/wilderness
    let mut roles: Vec<SystemRole> = Vec::with_capacity(system_count);

    // One capital per civ.
    for i in 0..num_civs {
        roles.push(SystemRole::Capital(i));
    }
    // One contested hub (start system).
    roles.push(SystemRole::Hub);

    // Distribute remaining systems.
    let remaining = system_count.saturating_sub(roles.len());
    let frontier_count = ((remaining as f64) * t.generation_rules.unclaimed_fraction).round() as usize;
    let territory_count = remaining.saturating_sub(frontier_count);

    // Territory: alternate between civs.
    for i in 0..territory_count {
        let civ_idx = i % num_civs;
        if i < num_civs {
            roles.push(SystemRole::Core(civ_idx));
        } else {
            roles.push(SystemRole::Colony(civ_idx));
        }
    }
    // Frontier and wilderness.
    for i in 0..frontier_count {
        if i < frontier_count / 2 {
            roles.push(SystemRole::Frontier);
        } else {
            roles.push(SystemRole::Wilderness);
        }
    }

    // Shuffle non-capital, non-hub entries for variety.
    let fixed_count = num_civs + 1; // capitals + hub
    roles[fixed_count..].shuffle(rng);

    // --- Generate positions ---
    let positions = generate_positions(rng, &roles, num_civs, spread,
        t.generation_rules.min_distance_between_systems);

    // --- Generate names ---
    let names = generate_system_names(rng, &t, system_count);

    // --- Generate star types ---
    let star_types_vec = generate_star_types(rng, &t, system_count);

    // --- Assemble systems ---
    let mut systems = Vec::with_capacity(system_count);

    for i in 0..system_count {
        let role = roles[i];
        let controlling_civ = match role {
            SystemRole::Capital(idx)
            | SystemRole::Core(idx)
            | SystemRole::Colony(idx) => Some(civ_ids[idx]),
            _ => None,
        };

        let infrastructure_level = match role {
            SystemRole::Capital(_) => InfrastructureLevel::Capital,
            SystemRole::Core(_) => InfrastructureLevel::Established,
            SystemRole::Colony(_) => InfrastructureLevel::Colony,
            SystemRole::Hub => InfrastructureLevel::Hub,
            SystemRole::Frontier => InfrastructureLevel::Outpost,
            SystemRole::Wilderness => InfrastructureLevel::None,
        };

        let time_factor = match role {
            SystemRole::Capital(_) | SystemRole::Hub | SystemRole::Core(_) => 1.0,
            SystemRole::Colony(_) => 1.0 + rng.gen_range(0.0..0.5),
            SystemRole::Frontier => rng.gen_range(
                t.generation_rules.time_factor_frontier_min
                ..=t.generation_rules.time_factor_frontier_max
            ),
            SystemRole::Wilderness => rng.gen_range(
                t.generation_rules.time_factor_deep_frontier_min
                ..=t.generation_rules.time_factor_deep_frontier_max
            ),
        };

        let star_type = star_types_vec[i];
        let name = &names[i];
        let planets = generate_planets(name, star_type, rng);

        let history = if infrastructure_level != InfrastructureLevel::None {
            vec![HistoryEntry {
                timestamp: Timestamp::zero(),
                description: format!("{} founded.", name),
            }]
        } else {
            vec![]
        };

        systems.push(StarSystem {
            id: Uuid::new_v4(),
            name: name.clone(),
            position: positions[i],
            star_type,
            planetary_bodies: planets,
            controlling_civ,
            infrastructure_level,
            history,
            active_threads: vec![],
            time_factor,
            faction_presence: vec![],
            economy: None,
        });
    }

    systems
}

/// Generate positions with spatial clustering around civ capitals.
fn generate_positions(
    rng: &mut StdRng,
    roles: &[SystemRole],
    num_civs: usize,
    spread: f64,
    min_dist: f64,
) -> Vec<(f64, f64)> {
    let mut positions: Vec<(f64, f64)> = Vec::with_capacity(roles.len());

    // Place capitals in a rough circle around center, well-separated.
    let center = (spread * 0.4, spread * 0.3);
    let capital_radius = spread * 0.3;

    let mut capital_positions: Vec<(f64, f64)> = Vec::new();
    for i in 0..num_civs {
        let angle = (i as f64 / num_civs as f64) * std::f64::consts::TAU
            + rng.gen_range(-0.3..0.3);
        let r = capital_radius + rng.gen_range(-2.0..2.0);
        let pos = (
            center.0 + r * angle.cos(),
            center.1 + r * angle.sin(),
        );
        capital_positions.push(pos);
    }

    // Hub goes near the center, between capitals.
    let hub_pos = (
        center.0 + rng.gen_range(-2.0..2.0),
        center.1 + rng.gen_range(-2.0..2.0),
    );

    for role in roles {
        let pos = match role {
            SystemRole::Capital(idx) => capital_positions[*idx],
            SystemRole::Hub => hub_pos,
            SystemRole::Core(idx) | SystemRole::Colony(idx) => {
                // Cluster near parent capital with increasing distance.
                let cap = capital_positions[*idx];
                let cluster_radius = match role {
                    SystemRole::Core(_) => 4.0,
                    _ => 7.0,
                };
                place_near(rng, cap, cluster_radius, min_dist, &positions)
            }
            SystemRole::Frontier => {
                // Place on the edges of the map.
                let angle: f64 = rng.gen_range(0.0..std::f64::consts::TAU);
                let r = spread * 0.4 + rng.gen_range(0.0..spread * 0.15);
                let candidate = (center.0 + r * angle.cos(), center.1 + r * angle.sin());
                nudge_if_too_close(rng, candidate, min_dist, &positions)
            }
            SystemRole::Wilderness => {
                // Deep frontier — furthest out.
                let angle: f64 = rng.gen_range(0.0..std::f64::consts::TAU);
                let r = spread * 0.45 + rng.gen_range(0.0..spread * 0.2);
                let candidate = (center.0 + r * angle.cos(), center.1 + r * angle.sin());
                nudge_if_too_close(rng, candidate, min_dist, &positions)
            }
        };
        positions.push(pos);
    }

    positions
}

/// Place a system near a target point, avoiding existing positions.
fn place_near(
    rng: &mut StdRng,
    target: (f64, f64),
    radius: f64,
    min_dist: f64,
    existing: &[(f64, f64)],
) -> (f64, f64) {
    for _ in 0..50 {
        let angle: f64 = rng.gen_range(0.0..std::f64::consts::TAU);
        let r = rng.gen_range(min_dist..radius);
        let candidate = (target.0 + r * angle.cos(), target.1 + r * angle.sin());
        if existing.iter().all(|p| dist2d(*p, candidate) >= min_dist) {
            return candidate;
        }
    }
    // Fallback: just place it with some offset.
    (target.0 + rng.gen_range(-radius..radius),
     target.1 + rng.gen_range(-radius..radius))
}

fn nudge_if_too_close(
    rng: &mut StdRng,
    candidate: (f64, f64),
    min_dist: f64,
    existing: &[(f64, f64)],
) -> (f64, f64) {
    if existing.iter().all(|p| dist2d(*p, candidate) >= min_dist) {
        return candidate;
    }
    // Nudge in a random direction.
    let angle: f64 = rng.gen_range(0.0..std::f64::consts::TAU);
    (candidate.0 + min_dist * angle.cos(),
     candidate.1 + min_dist * angle.sin())
}

fn dist2d(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    (dx * dx + dy * dy).sqrt()
}

/// Generate unique system names from template pools.
fn generate_system_names(
    rng: &mut StdRng,
    t: &templates::SystemTemplates,
    count: usize,
) -> Vec<String> {
    let mut names: Vec<String> = Vec::with_capacity(count);
    let mut used_standalones: Vec<usize> = Vec::new();

    for i in 0..count {
        // Mix of name types: 50% standalone, 30% compound, 20% explorer.
        let roll: f64 = rng.gen();
        let name = if roll < 0.50 {
            pick_standalone_name(rng, t, &used_standalones, &mut names)
                .map(|idx| { used_standalones.push(idx); t.standalone_names[idx].name.clone() })
                .unwrap_or_else(|| fallback_name(rng, i))
        } else if roll < 0.80 {
            generate_compound_name(rng, t, &names)
                .unwrap_or_else(|| fallback_name(rng, i))
        } else {
            generate_explorer_name(rng, t, &names)
                .unwrap_or_else(|| fallback_name(rng, i))
        };

        names.push(name);
    }

    names
}

fn pick_standalone_name(
    rng: &mut StdRng,
    t: &templates::SystemTemplates,
    used: &[usize],
    existing: &[String],
) -> Option<usize> {
    let available: Vec<(usize, f64)> = t.standalone_names.iter().enumerate()
        .filter(|(i, n)| !used.contains(i) && !existing.contains(&n.name))
        .map(|(i, n)| (i, n.weight))
        .collect();
    if available.is_empty() {
        return None;
    }
    let total: f64 = available.iter().map(|(_, w)| w).sum();
    let mut roll: f64 = rng.gen::<f64>() * total;
    for (idx, weight) in &available {
        roll -= weight;
        if roll <= 0.0 {
            return Some(*idx);
        }
    }
    Some(available.last().unwrap().0)
}

fn generate_compound_name(
    rng: &mut StdRng,
    t: &templates::SystemTemplates,
    existing: &[String],
) -> Option<String> {
    for _ in 0..20 {
        let prefix = weighted_pick(rng, &t.compound_prefixes);
        let suffix = weighted_pick(rng, &t.compound_suffixes);
        let name = format!("{} {}", prefix.name, suffix.name);
        if !existing.contains(&name) {
            return Some(name);
        }
    }
    None
}

fn generate_explorer_name(
    rng: &mut StdRng,
    t: &templates::SystemTemplates,
    existing: &[String],
) -> Option<String> {
    for _ in 0..20 {
        let surname = weighted_pick_wn(rng, &t.explorer_surnames);
        let suffix = weighted_pick_wn(rng, &t.explorer_suffixes);
        let name = format!("{}{}", surname.name, suffix.name);
        if !existing.contains(&name) {
            return Some(name);
        }
    }
    None
}

fn weighted_pick<'a>(rng: &mut StdRng, entries: &'a [templates::NameEntry]) -> &'a templates::NameEntry {
    let total: f64 = entries.iter().map(|e| e.weight).sum();
    let mut roll = rng.gen::<f64>() * total;
    for entry in entries {
        roll -= entry.weight;
        if roll <= 0.0 {
            return entry;
        }
    }
    entries.last().unwrap()
}

fn weighted_pick_wn<'a>(rng: &mut StdRng, entries: &'a [templates::WeightedName]) -> &'a templates::WeightedName {
    let total: f64 = entries.iter().map(|e| e.weight).sum();
    let mut roll = rng.gen::<f64>() * total;
    for entry in entries {
        roll -= entry.weight;
        if roll <= 0.0 {
            return entry;
        }
    }
    entries.last().unwrap()
}

fn fallback_name(rng: &mut StdRng, index: usize) -> String {
    format!("System {}-{}", index + 1, rng.gen_range(100..999))
}

/// Generate star types from weighted template pool.
fn generate_star_types(
    rng: &mut StdRng,
    t: &templates::SystemTemplates,
    count: usize,
) -> Vec<StarType> {
    (0..count).map(|_| {
        let total: f64 = t.star_types.iter().map(|s| s.weight).sum();
        let mut roll = rng.gen::<f64>() * total;
        for entry in &t.star_types {
            roll -= entry.weight;
            if roll <= 0.0 {
                return parse_star_type(&entry.star_type);
            }
        }
        StarType::RedDwarf // fallback
    }).collect()
}

fn parse_star_type(s: &str) -> StarType {
    match s {
        "yellow_dwarf" => StarType::YellowDwarf,
        "red_dwarf" => StarType::RedDwarf,
        "binary" => StarType::Binary,
        "blue_giant" => StarType::BlueGiant,
        "white_dwarf" => StarType::WhiteDwarf,
        "neutron" => StarType::Neutron,
        "black_hole" => StarType::BlackHole,
        "anomalous" => StarType::Anomalous,
        _ => StarType::RedDwarf,
    }
}

fn generate_planets(
    system_name: &str,
    star_type: StarType,
    rng: &mut StdRng,
) -> Vec<PlanetaryBody> {
    let count = match star_type {
        StarType::Neutron | StarType::BlackHole => rng.gen_range(0..=1),
        StarType::BlueGiant => rng.gen_range(1..=3),
        StarType::Anomalous => rng.gen_range(0..=2),
        _ => rng.gen_range(1..=5),
    };

    let body_types = [
        BodyType::Terrestrial,
        BodyType::GasGiant,
        BodyType::IceWorld,
        BodyType::Barren,
        BodyType::Oceanic,
    ];

    (0..count)
        .map(|i| {
            let body_type = body_types[rng.gen_range(0..body_types.len())];
            PlanetaryBody {
                name: format!("{} {}", system_name, roman_numeral(i + 1)),
                body_type,
                features: vec![],
            }
        })
        .collect()
}

// ===========================================================================
// Connection generation
// ===========================================================================

fn generate_connections(systems: &[StarSystem], rng: &mut StdRng) -> Vec<Connection> {
    let mut connections: Vec<Connection> = Vec::new();
    let mut connected_pairs: Vec<(Uuid, Uuid)> = Vec::new();

    let has_edge = |pairs: &[(Uuid, Uuid)], a: Uuid, b: Uuid| -> bool {
        pairs
            .iter()
            .any(|(x, y)| (*x == a && *y == b) || (*x == b && *y == a))
    };

    // Phase 1: connect each system to its nearest neighbor (minimum spanning).
    for i in 0..systems.len() {
        let mut nearest_idx = if i == 0 { 1 } else { 0 };
        let mut nearest_dist = distance(&systems[i], &systems[nearest_idx]);

        for j in 0..systems.len() {
            if j == i {
                continue;
            }
            let d = distance(&systems[i], &systems[j]);
            if d < nearest_dist {
                nearest_dist = d;
                nearest_idx = j;
            }
        }

        if !has_edge(&connected_pairs, systems[i].id, systems[nearest_idx].id) {
            let route = classify_route(nearest_dist, rng);
            connections.push(Connection {
                system_a: systems[i].id,
                system_b: systems[nearest_idx].id,
                distance_ly: nearest_dist,
                route_type: route,
            });
            connected_pairs.push((systems[i].id, systems[nearest_idx].id));
        }
    }

    // Phase 2: add all connections within threshold distance.
    let threshold = 12.0;
    for i in 0..systems.len() {
        for j in (i + 1)..systems.len() {
            if has_edge(&connected_pairs, systems[i].id, systems[j].id) {
                continue;
            }
            let d = distance(&systems[i], &systems[j]);
            if d <= threshold {
                let route = classify_route(d, rng);
                connections.push(Connection {
                    system_a: systems[i].id,
                    system_b: systems[j].id,
                    distance_ly: d,
                    route_type: route,
                });
                connected_pairs.push((systems[i].id, systems[j].id));
            }
        }
    }

    // Phase 3: ensure a corridor between capital systems for narrative purposes.
    let capitals: Vec<&StarSystem> = systems.iter()
        .filter(|s| s.infrastructure_level == InfrastructureLevel::Capital)
        .collect();
    for i in 0..capitals.len() {
        for j in (i + 1)..capitals.len() {
            if !has_edge(&connected_pairs, capitals[i].id, capitals[j].id) {
                let d = distance(capitals[i], capitals[j]);
                connections.push(Connection {
                    system_a: capitals[i].id,
                    system_b: capitals[j].id,
                    distance_ly: d,
                    route_type: RouteType::Corridor,
                });
                connected_pairs.push((capitals[i].id, capitals[j].id));
            }
        }
    }

    connections
}

fn distance(a: &StarSystem, b: &StarSystem) -> f64 {
    let dx = a.position.0 - b.position.0;
    let dy = a.position.1 - b.position.1;
    (dx * dx + dy * dy).sqrt()
}

fn classify_route(distance_ly: f64, rng: &mut StdRng) -> RouteType {
    if distance_ly > 15.0 {
        RouteType::Corridor
    } else if rng.gen_bool(0.15) {
        RouteType::Hazardous
    } else {
        RouteType::Open
    }
}

fn roman_numeral(n: usize) -> &'static str {
    match n {
        1 => "I",
        2 => "II",
        3 => "III",
        4 => "IV",
        5 => "V",
        _ => "VI",
    }
}

// ===========================================================================
// Economy generation
// ===========================================================================

/// Economy archetypes based on infrastructure and planet composition.
/// Each archetype defines production/consumption biases.
#[derive(Debug, Clone, Copy)]
enum EconomyArchetype {
    /// Agricultural world — produces food, consumes manufactured goods.
    Agricultural,
    /// Mining/extraction — produces raw materials, consumes food and medical.
    Extraction,
    /// Manufacturing hub — produces manufactured goods, consumes raw materials.
    Manufacturing,
    /// Trade hub — moderate everything, low volatility.
    TradeHub,
    /// Military outpost — consumes heavily, produces little.
    Military,
    /// Frontier — limited everything, high volatility.
    Frontier,
}

/// Assign economy profiles to inhabited systems (Outpost+).
fn assign_system_economies(
    systems: &mut [StarSystem],
    factions: &[Faction],
    rng: &mut StdRng,
) {
    for system in systems.iter_mut() {
        let infra = system.infrastructure_level;
        if infra == InfrastructureLevel::None {
            continue;
        }

        // Pick archetype based on infrastructure + randomness.
        let archetype = match infra {
            InfrastructureLevel::Outpost => {
                // Outposts are frontier or extraction.
                if rng.gen_bool(0.6) {
                    EconomyArchetype::Frontier
                } else {
                    EconomyArchetype::Extraction
                }
            }
            InfrastructureLevel::Colony => {
                // Colonies specialize.
                *[EconomyArchetype::Agricultural, EconomyArchetype::Extraction,
                  EconomyArchetype::Manufacturing]
                    .choose(rng).unwrap()
            }
            InfrastructureLevel::Established => {
                // Established systems lean toward manufacturing or trade.
                *[EconomyArchetype::Manufacturing, EconomyArchetype::TradeHub,
                  EconomyArchetype::Agricultural]
                    .choose(rng).unwrap()
            }
            InfrastructureLevel::Hub => EconomyArchetype::TradeHub,
            InfrastructureLevel::Capital => {
                // Capitals are trade hubs or manufacturing centers.
                if rng.gen_bool(0.5) {
                    EconomyArchetype::TradeHub
                } else {
                    EconomyArchetype::Manufacturing
                }
            }
            InfrastructureLevel::None => unreachable!(),
        };

        // Check if military faction dominance shifts the archetype.
        let has_strong_military = system.faction_presence.iter().any(|fp| {
            fp.strength >= 0.6
                && factions.iter()
                    .find(|f| f.id == fp.faction_id)
                    .map(|f| f.category == FactionCategory::Military)
                    .unwrap_or(false)
        });
        let archetype = if has_strong_military && rng.gen_bool(0.4) {
            EconomyArchetype::Military
        } else {
            archetype
        };

        let mut production = HashMap::new();
        let mut consumption = HashMap::new();

        // Base profiles from archetype, with small random variance.
        let variance = |rng: &mut StdRng, base: f32| -> f32 {
            (base + rng.gen_range(-0.1..0.1)).clamp(0.0, 1.0)
        };

        match archetype {
            EconomyArchetype::Agricultural => {
                production.insert(TradeGood::Food, variance(rng, 0.8));
                production.insert(TradeGood::RawMaterials, variance(rng, 0.3));
                production.insert(TradeGood::ManufacturedGoods, variance(rng, 0.1));
                production.insert(TradeGood::MedicalSupplies, variance(rng, 0.3));
                consumption.insert(TradeGood::ManufacturedGoods, variance(rng, 0.7));
                consumption.insert(TradeGood::ConstructionMaterials, variance(rng, 0.5));
                consumption.insert(TradeGood::RefinedFuelCells, variance(rng, 0.4));
            }
            EconomyArchetype::Extraction => {
                production.insert(TradeGood::RawMaterials, variance(rng, 0.9));
                production.insert(TradeGood::ConstructionMaterials, variance(rng, 0.5));
                consumption.insert(TradeGood::Food, variance(rng, 0.7));
                consumption.insert(TradeGood::MedicalSupplies, variance(rng, 0.6));
                consumption.insert(TradeGood::ManufacturedGoods, variance(rng, 0.5));
                consumption.insert(TradeGood::RefinedFuelCells, variance(rng, 0.3));
            }
            EconomyArchetype::Manufacturing => {
                production.insert(TradeGood::ManufacturedGoods, variance(rng, 0.8));
                production.insert(TradeGood::RefinedFuelCells, variance(rng, 0.5));
                consumption.insert(TradeGood::RawMaterials, variance(rng, 0.8));
                consumption.insert(TradeGood::Food, variance(rng, 0.5));
                consumption.insert(TradeGood::ConstructionMaterials, variance(rng, 0.3));
            }
            EconomyArchetype::TradeHub => {
                // Trade hubs have moderate everything — they're middlemen.
                for good in TradeGood::all() {
                    production.insert(*good, variance(rng, 0.4));
                    consumption.insert(*good, variance(rng, 0.4));
                }
            }
            EconomyArchetype::Military => {
                production.insert(TradeGood::RefinedFuelCells, variance(rng, 0.4));
                consumption.insert(TradeGood::Food, variance(rng, 0.6));
                consumption.insert(TradeGood::RawMaterials, variance(rng, 0.5));
                consumption.insert(TradeGood::ManufacturedGoods, variance(rng, 0.7));
                consumption.insert(TradeGood::MedicalSupplies, variance(rng, 0.6));
                consumption.insert(TradeGood::ConstructionMaterials, variance(rng, 0.6));
                consumption.insert(TradeGood::RefinedFuelCells, variance(rng, 0.7));
            }
            EconomyArchetype::Frontier => {
                // Frontier systems have very little — high consumption, low production.
                for good in TradeGood::all() {
                    production.insert(*good, variance(rng, 0.1));
                    consumption.insert(*good, variance(rng, 0.5));
                }
                // But they might have raw materials from local extraction.
                production.insert(TradeGood::RawMaterials, variance(rng, 0.4));
            }
        }

        let price_volatility = match infra {
            InfrastructureLevel::Capital => 0.6,
            InfrastructureLevel::Hub => 0.8,
            InfrastructureLevel::Established => 1.0,
            InfrastructureLevel::Colony => 1.2,
            InfrastructureLevel::Outpost => 1.8,
            InfrastructureLevel::None => 2.0,
        };

        let fuel_price = match infra {
            InfrastructureLevel::Capital | InfrastructureLevel::Hub => 2.0 + rng.gen_range(0.0..1.0),
            InfrastructureLevel::Established | InfrastructureLevel::Colony => 3.0 + rng.gen_range(0.0..2.0),
            InfrastructureLevel::Outpost => 5.0 + rng.gen_range(0.0..3.0),
            InfrastructureLevel::None => 0.0,
        };

        let supply_price = match infra {
            InfrastructureLevel::Capital | InfrastructureLevel::Hub => 1.5 + rng.gen_range(0.0..0.5),
            InfrastructureLevel::Established | InfrastructureLevel::Colony => 2.0 + rng.gen_range(0.0..1.5),
            InfrastructureLevel::Outpost => 3.5 + rng.gen_range(0.0..2.0),
            InfrastructureLevel::None => 0.0,
        };

        system.economy = Some(SystemEconomy {
            production,
            consumption,
            price_volatility,
            fuel_price,
            supply_price,
        });
    }
}

// ===========================================================================
// NPC generation
// ===========================================================================

/// First names and last names for NPC generation.
const NPC_FIRST_NAMES: [&str; 20] = [
    "Maren", "Joss", "Kael", "Suri", "Dav", "Ren", "Thea", "Orin",
    "Lys", "Cade", "Nessa", "Tomas", "Asha", "Vek", "Petra", "Idris",
    "Yara", "Leong", "Sienna", "Harlan",
];

const NPC_LAST_NAMES: [&str; 20] = [
    "Solari", "Voss", "Kessler", "Okafor", "Tannen", "Haig", "Reyes",
    "Strand", "Vasil", "Torren", "Ashcroft", "Nazari", "Brennan", "Loh",
    "Duval", "Mikkelsen", "Achebe", "Cross", "Sato", "Kaur",
];

/// Title and bio template per faction category.
fn npc_template_for_category(category: FactionCategory) -> (&'static str, &'static str) {
    match category {
        FactionCategory::Military => (
            "Garrison Commander",
            "Career officer posted here to maintain order. Efficient, formal, \
             evaluates everyone by whether they're useful or a problem.",
        ),
        FactionCategory::Economic => (
            "Trade Liaison",
            "Manages commercial operations and trade agreements. Knows the price \
             of everything and the value of reliable partners.",
        ),
        FactionCategory::Guild => (
            "Guild Factor",
            "The local representative of the Corridor Guild. Handles contracts, \
             repairs, and the quiet logistics that keep ships moving.",
        ),
        FactionCategory::Criminal => (
            "Fixer",
            "Operates in the spaces between official channels. Knows who needs \
             what moved and who doesn't ask questions.",
        ),
        FactionCategory::Religious => (
            "Prior",
            "Tends to the spiritual needs of travelers and locals. Speaks carefully, \
             listens more carefully, knows things about distorted space.",
        ),
        FactionCategory::Academic => (
            "Research Coordinator",
            "Manages scientific operations and data collection. Perpetually underfunded, \
             perpetually curious.",
        ),
        FactionCategory::Political => (
            "Administrator",
            "The local face of governance. Manages disputes, allocates resources, \
             and tries to keep everyone from each other's throats.",
        ),
    }
}

/// Generate permanent NPCs for systems with Colony+ infrastructure.
/// One NPC per major faction presence at each qualifying system.
fn generate_npcs(
    systems: &[StarSystem],
    factions: &[Faction],
    rng: &mut StdRng,
) -> Vec<Npc> {
    let mut npcs = Vec::new();
    let mut name_idx = 0usize;

    for system in systems {
        // Only systems with real infrastructure get NPCs.
        let infra_rank = match system.infrastructure_level {
            InfrastructureLevel::None | InfrastructureLevel::Outpost => continue,
            InfrastructureLevel::Colony => 1,
            InfrastructureLevel::Established => 2,
            InfrastructureLevel::Hub => 3,
            InfrastructureLevel::Capital => 3,
        };

        // Pick the top faction presences by strength, up to infra_rank.
        let mut presences: Vec<&FactionPresence> = system.faction_presence.iter()
            .filter(|fp| fp.strength >= 0.3)
            .collect();
        presences.sort_by(|a, b| b.strength.partial_cmp(&a.strength).unwrap());
        presences.truncate(infra_rank);

        for presence in presences {
            let faction = match factions.iter().find(|f| f.id == presence.faction_id) {
                Some(f) => f,
                None => continue,
            };

            let (title, bio) = npc_template_for_category(faction.category);

            // Deterministic name selection using a rotating index + rng.
            let first = NPC_FIRST_NAMES[name_idx % NPC_FIRST_NAMES.len()];
            let last_offset: usize = rng.gen_range(0..NPC_LAST_NAMES.len());
            let last = NPC_LAST_NAMES[(name_idx + last_offset) % NPC_LAST_NAMES.len()];
            name_idx += 1;

            let mut npc = Npc::new(
                format!("{} {}", first, last),
                title,
                Some(faction.id),
                system.id,
                bio,
            );

            // Add a motivation based on faction category.
            npc.motivations.push(match faction.category {
                FactionCategory::Military => "maintain order and security".into(),
                FactionCategory::Economic => "expand trade routes and profits".into(),
                FactionCategory::Guild => "keep ships flying and contracts honored".into(),
                FactionCategory::Criminal => "move goods, avoid attention".into(),
                FactionCategory::Religious => "understand the distortions".into(),
                FactionCategory::Academic => "gather data, publish findings".into(),
                FactionCategory::Political => "keep the peace, grow influence".into(),
            });

            npcs.push(npc);
        }
    }

    npcs
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn find_faction_by_category<'a>(
        factions: &'a [Faction],
        cat: FactionCategory,
    ) -> &'a Faction {
        factions.iter().find(|f| f.category == cat).unwrap()
    }

    fn find_criminal_frontier<'a>(factions: &'a [Faction]) -> &'a Faction {
        factions
            .iter()
            .find(|f| {
                f.category == FactionCategory::Criminal
                    && matches!(f.scope, FactionScope::Independent)
            })
            .unwrap()
    }

    #[allow(dead_code)]
    fn find_criminal_covert<'a>(factions: &'a [Faction]) -> &'a Faction {
        factions
            .iter()
            .find(|f| {
                f.category == FactionCategory::Criminal
                    && matches!(f.scope, FactionScope::Transnational { .. })
            })
            .unwrap()
    }

    // -----------------------------------------------------------------------
    // Galaxy-level
    // -----------------------------------------------------------------------

    #[test]
    fn generates_consistent_galaxy_from_seed() {
        let g1 = generate_galaxy(42);
        let g2 = generate_galaxy(42);

        assert_eq!(g1.systems.len(), 10);
        assert_eq!(g1.civilizations.len(), g2.civilizations.len());

        for (a, b) in g1.systems.iter().zip(g2.systems.iter()) {
            assert_eq!(a.name, b.name);
        }

        let mut names1: Vec<&str> = g1.civilizations.iter().map(|c| c.name.as_str()).collect();
        let mut names2: Vec<&str> = g2.civilizations.iter().map(|c| c.name.as_str()).collect();
        names1.sort();
        names2.sort();
        assert_eq!(names1, names2);
    }

    #[test]
    fn all_systems_have_at_least_one_connection() {
        let galaxy = generate_galaxy(123);
        for sys in &galaxy.systems {
            let has_conn = galaxy
                .connections
                .iter()
                .any(|c| c.system_a == sys.id || c.system_b == sys.id);
            assert!(has_conn, "System {} has no connections", sys.name);
        }
    }

    #[test]
    fn different_seeds_produce_different_positions() {
        let g1 = generate_galaxy(1);
        let g2 = generate_galaxy(2);
        let pos_differ = g1
            .systems
            .iter()
            .zip(g2.systems.iter())
            .any(|(a, b)| a.position != b.position);
        assert!(pos_differ, "Different seeds should produce different positions");
    }

    #[test]
    fn different_seeds_produce_different_civ_names() {
        let mut found_different = false;
        for seed in [1, 2, 3, 10, 42, 100, 999] {
            let g1 = generate_galaxy(seed);
            let g2 = generate_galaxy(seed + 7);
            let names1: HashSet<&str> = g1.civilizations.iter().map(|c| c.name.as_str()).collect();
            let names2: HashSet<&str> = g2.civilizations.iter().map(|c| c.name.as_str()).collect();
            if names1 != names2 {
                found_different = true;
                break;
            }
        }
        assert!(found_different, "Different seeds should sometimes produce different civ names");
    }

    // -----------------------------------------------------------------------
    // Civilizations
    // -----------------------------------------------------------------------

    #[test]
    fn civ_count_within_expected_range() {
        for seed in [1, 42, 100, 999] {
            let galaxy = generate_galaxy(seed);
            let count = galaxy.civilizations.len();
            assert!(
                (2..=5).contains(&count),
                "Seed {} produced {} civs (expected 2-5)",
                seed, count,
            );
        }
    }

    #[test]
    fn no_duplicate_civ_suffixes() {
        for seed in [1, 42, 100, 999] {
            let galaxy = generate_galaxy(seed);
            let suffixes: Vec<&str> = galaxy
                .civilizations
                .iter()
                .map(|c| {
                    let name = c.name.strip_prefix("The ").unwrap_or(&c.name);
                    name.split_whitespace().last().unwrap()
                })
                .collect();
            let unique: HashSet<&&str> = suffixes.iter().collect();
            assert_eq!(
                suffixes.len(), unique.len(),
                "Seed {} has duplicate civ suffixes: {:?}", seed, suffixes,
            );
        }
    }

    #[test]
    fn civ_ethos_values_in_range() {
        let galaxy = generate_galaxy(42);
        for civ in &galaxy.civilizations {
            let vals = [
                civ.ethos.expansionist, civ.ethos.isolationist,
                civ.ethos.militaristic, civ.ethos.diplomatic,
                civ.ethos.theocratic, civ.ethos.mercantile,
                civ.ethos.technocratic, civ.ethos.communal,
            ];
            for v in &vals {
                assert!(
                    (0.0..=1.0).contains(v),
                    "Civ '{}' has ethos value {} out of range", civ.name, v,
                );
            }
        }
    }

    #[test]
    fn civ_capabilities_in_range() {
        let galaxy = generate_galaxy(42);
        for civ in &galaxy.civilizations {
            for v in &[civ.capabilities.size, civ.capabilities.wealth,
                       civ.capabilities.technology, civ.capabilities.military] {
                assert!(
                    (0.0..=1.0).contains(v),
                    "Civ '{}' has capability {} out of range", civ.name, v,
                );
            }
        }
    }

    #[test]
    fn civ_relationships_are_mutual() {
        let galaxy = generate_galaxy(42);
        for civ in &galaxy.civilizations {
            for (&other_id, _) in &civ.relationships {
                let other = galaxy.civilizations.iter().find(|c| c.id == other_id).unwrap();
                assert!(
                    other.relationships.contains_key(&civ.id),
                    "Civ '{}' has relationship with '{}' but not vice versa",
                    civ.name, other.name,
                );
            }
        }
    }

    #[test]
    fn every_civ_has_pressures() {
        let galaxy = generate_galaxy(42);
        for civ in &galaxy.civilizations {
            assert!(
                !civ.internal_dynamics.pressures.is_empty(),
                "Civ '{}' has no internal pressures", civ.name,
            );
        }
    }

    #[test]
    fn no_blocked_pairs_in_civ_names() {
        let t = templates::load_civ_templates();
        for seed in [1, 42, 100, 999] {
            let galaxy = generate_galaxy(seed);
            for civ in &galaxy.civilizations {
                let name = civ.name.strip_prefix("The ").unwrap_or(&civ.name);
                let parts: Vec<&str> = name.split_whitespace().collect();
                if parts.len() >= 2 {
                    let prefix = parts[0];
                    let suffix = parts[parts.len() - 1];
                    assert!(
                        !t.compatibility.is_blocked(prefix, suffix),
                        "Seed {} produced blocked pair: '{}' + '{}'",
                        seed, prefix, suffix,
                    );
                }
            }
        }
    }

    #[test]
    fn civ_names_look_reasonable() {
        for seed in [1, 42, 100] {
            let galaxy = generate_galaxy(seed);
            for civ in &galaxy.civilizations {
                let name = civ.name.strip_prefix("The ").unwrap_or(&civ.name);
                let words: Vec<&str> = name.split_whitespace().collect();
                assert!(
                    words.len() >= 2,
                    "Civ name '{}' should have at least prefix + suffix", civ.name,
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // System assignments
    // -----------------------------------------------------------------------

    #[test]
    fn civ_assignments_are_sensible() {
        let galaxy = generate_galaxy(42);

        for civ in &galaxy.civilizations {
            let count = galaxy.systems.iter()
                .filter(|s| s.controlling_civ == Some(civ.id))
                .count();
            assert!(count >= 1, "Civ '{}' controls no systems", civ.name);
        }

        let assigned: usize = galaxy.systems.iter()
            .filter(|s| s.controlling_civ.is_some())
            .count();
        let unclaimed = galaxy.systems.len() - assigned;
        assert_eq!(galaxy.systems.len(), 10);
        assert!(unclaimed >= 3, "Expected at least 3 unclaimed systems, got {}", unclaimed);
    }

    #[test]
    fn time_factors_assigned_correctly() {
        let galaxy = generate_galaxy(42);

        // Capitals should have normal time.
        for sys in galaxy.systems.iter().filter(|s| s.infrastructure_level == InfrastructureLevel::Capital) {
            assert_eq!(sys.time_factor, 1.0, "Capital {} should have time_factor 1.0", sys.name);
        }

        // Hub should have normal time.
        for sys in galaxy.systems.iter().filter(|s| s.infrastructure_level == InfrastructureLevel::Hub) {
            assert_eq!(sys.time_factor, 1.0, "Hub {} should have time_factor 1.0", sys.name);
        }

        // Wilderness systems (None infra) should have high distortion.
        for sys in galaxy.systems.iter().filter(|s| s.infrastructure_level == InfrastructureLevel::None) {
            assert!(sys.time_factor >= 4.0,
                "Wilderness {} should have high distortion, got {}", sys.name, sys.time_factor);
        }
    }

    // -----------------------------------------------------------------------
    // Factions (structural)
    // -----------------------------------------------------------------------

    #[test]
    fn generates_at_least_guaranteed_factions() {
        let galaxy = generate_galaxy(42);
        // Guaranteed: military, economic, guild, religious, criminal_frontier (5)
        assert!(galaxy.factions.len() >= 5,
            "Should generate at least 5 guaranteed factions, got {}", galaxy.factions.len());
    }

    #[test]
    fn faction_generation_is_deterministic() {
        let g1 = generate_galaxy(42);
        let g2 = generate_galaxy(42);
        assert_eq!(g1.factions.len(), g2.factions.len());

        let mut names1: Vec<&str> = g1.factions.iter().map(|f| f.name.as_str()).collect();
        let mut names2: Vec<&str> = g2.factions.iter().map(|f| f.name.as_str()).collect();
        names1.sort();
        names2.sort();
        assert_eq!(names1, names2);
    }

    #[test]
    fn all_faction_ids_are_unique() {
        let galaxy = generate_galaxy(42);
        let ids: HashSet<Uuid> = galaxy.factions.iter().map(|f| f.id).collect();
        assert_eq!(ids.len(), galaxy.factions.len());
    }

    #[test]
    fn faction_categories_are_diverse() {
        let galaxy = generate_galaxy(42);
        let categories: HashSet<FactionCategory> = galaxy.factions.iter().map(|f| f.category).collect();
        assert!(categories.len() >= 4, "Factions should span at least 4 categories, got {}", categories.len());
    }

    #[test]
    fn faction_ethos_values_in_range() {
        let galaxy = generate_galaxy(42);
        for f in &galaxy.factions {
            assert!((-1.0..=1.0).contains(&f.ethos.alignment), "{} alignment out of range", f.name);
            assert!((0.0..=1.0).contains(&f.ethos.openness), "{} openness out of range", f.name);
            assert!((0.0..=1.0).contains(&f.ethos.aggression), "{} aggression out of range", f.name);
        }
    }

    #[test]
    fn faction_influence_references_valid_civ_ids() {
        let galaxy = generate_galaxy(42);
        let civ_ids: HashSet<Uuid> = galaxy.civilizations.iter().map(|c| c.id).collect();
        for f in &galaxy.factions {
            for cid in f.influence.keys() {
                assert!(civ_ids.contains(cid), "Faction {} references non-existent civ {}", f.name, cid);
            }
        }
    }

    #[test]
    fn faction_influence_values_in_range() {
        let galaxy = generate_galaxy(42);
        for f in &galaxy.factions {
            for (&cid, &val) in &f.influence {
                assert!((0.0..=1.0).contains(&val), "Faction {} influence {} in civ {}", f.name, val, cid);
            }
        }
    }

    #[test]
    fn factions_wired_into_civilizations() {
        let galaxy = generate_galaxy(42);
        for civ in &galaxy.civilizations {
            assert!(!civ.faction_ids.is_empty(), "Civ '{}' should have faction IDs", civ.name);
        }
        let faction_ids: HashSet<Uuid> = galaxy.factions.iter().map(|f| f.id).collect();
        for civ in &galaxy.civilizations {
            for fid in &civ.faction_ids {
                assert!(faction_ids.contains(fid), "Civ {} references non-existent faction {}", civ.name, fid);
            }
        }
    }

    #[test]
    fn independent_factions_not_in_any_civ() {
        let galaxy = generate_galaxy(42);
        let all_civ_fids: HashSet<Uuid> = galaxy.civilizations.iter()
            .flat_map(|c| c.faction_ids.iter()).copied().collect();
        for f in &galaxy.factions {
            if matches!(f.scope, FactionScope::Independent) {
                assert!(!all_civ_fids.contains(&f.id),
                    "Independent faction {} in a civ's faction_ids", f.name);
            }
        }
    }

    #[test]
    fn civ_internal_faction_only_in_parent_civ() {
        let galaxy = generate_galaxy(42);
        for f in &galaxy.factions {
            if let FactionScope::CivInternal { civ_id } = &f.scope {
                let parent = galaxy.civilizations.iter().find(|c| c.id == *civ_id).unwrap();
                assert!(parent.faction_ids.contains(&f.id),
                    "CivInternal faction {} should be in parent civ {}", f.name, parent.name);
                for civ in &galaxy.civilizations {
                    if civ.id != *civ_id {
                        assert!(!civ.faction_ids.contains(&f.id),
                            "CivInternal faction {} should NOT be in {}", f.name, civ.name);
                    }
                }
            }
        }
    }

    #[test]
    fn transnational_factions_in_all_listed_civs() {
        let galaxy = generate_galaxy(42);
        for f in &galaxy.factions {
            if let FactionScope::Transnational { civ_ids } = &f.scope {
                for cid in civ_ids {
                    let civ = galaxy.civilizations.iter().find(|c| c.id == *cid).unwrap();
                    assert!(civ.faction_ids.contains(&f.id),
                        "Transnational faction {} should be in civ {}", f.name, civ.name);
                }
            }
        }
    }

    #[test]
    fn pressure_sources_wired_to_valid_factions() {
        let galaxy = generate_galaxy(42);
        let fids: HashSet<Uuid> = galaxy.factions.iter().map(|f| f.id).collect();
        for civ in &galaxy.civilizations {
            for p in &civ.internal_dynamics.pressures {
                if let Some(sid) = p.source_faction {
                    assert!(fids.contains(&sid),
                        "Pressure '{}' in {} references non-existent faction {}", p.description, civ.name, sid);
                }
            }
        }
    }

    #[test]
    fn some_pressures_have_faction_sources() {
        let galaxy = generate_galaxy(42);
        let sourced: usize = galaxy.civilizations.iter()
            .flat_map(|c| c.internal_dynamics.pressures.iter())
            .filter(|p| p.source_faction.is_some())
            .count();
        assert!(sourced >= 1, "At least 1 pressure should be linked to a faction (got {})", sourced);
    }

    // -----------------------------------------------------------------------
    // Faction presence
    // -----------------------------------------------------------------------

    #[test]
    fn faction_presence_references_valid_faction_ids() {
        let galaxy = generate_galaxy(42);
        let fids: HashSet<Uuid> = galaxy.factions.iter().map(|f| f.id).collect();
        for sys in &galaxy.systems {
            for fp in &sys.faction_presence {
                assert!(fids.contains(&fp.faction_id),
                    "System {} has presence for non-existent faction {}", sys.name, fp.faction_id);
            }
        }
    }

    #[test]
    fn faction_presence_strength_and_visibility_in_range() {
        let galaxy = generate_galaxy(42);
        for sys in &galaxy.systems {
            for fp in &sys.faction_presence {
                assert!((0.0..=1.0).contains(&fp.strength),
                    "System {} presence strength out of range: {}", sys.name, fp.strength);
                assert!((0.0..=1.0).contains(&fp.visibility),
                    "System {} presence visibility out of range: {}", sys.name, fp.visibility);
            }
        }
    }

    #[test]
    fn every_system_has_faction_presence() {
        let galaxy = generate_galaxy(42);
        for sys in &galaxy.systems {
            assert!(!sys.faction_presence.is_empty(), "System {} has no faction presence", sys.name);
        }
    }

    #[test]
    fn factions_not_all_piled_into_one_system() {
        let galaxy = generate_galaxy(42);
        let mut count: HashMap<Uuid, usize> = HashMap::new();
        for sys in &galaxy.systems {
            for fp in &sys.faction_presence {
                *count.entry(fp.faction_id).or_insert(0) += 1;
            }
        }
        for f in &galaxy.factions {
            let c = count.get(&f.id).copied().unwrap_or(0);
            assert!(c >= 1, "Faction {} has no system presence", f.name);
            assert!(c < 10, "Faction {} is in all {} systems", f.name, c);
        }
    }

    #[test]
    fn no_duplicate_faction_presence_in_system() {
        let galaxy = generate_galaxy(42);
        for sys in &galaxy.systems {
            let ids: Vec<Uuid> = sys.faction_presence.iter().map(|fp| fp.faction_id).collect();
            let unique: HashSet<Uuid> = ids.iter().copied().collect();
            assert_eq!(ids.len(), unique.len(), "System {} has duplicate presence", sys.name);
        }
    }

    #[test]
    fn capital_has_strong_military_presence() {
        let galaxy = generate_galaxy(42);
        let capital = galaxy.systems.iter()
            .find(|s| s.infrastructure_level == InfrastructureLevel::Capital)
            .expect("Should have a capital");
        let mil = find_faction_by_category(&galaxy.factions, FactionCategory::Military);
        let mp = capital.faction_presence.iter().find(|fp| fp.faction_id == mil.id);
        assert!(mp.is_some(), "Capital {} should have military presence", capital.name);
        assert!(mp.unwrap().strength >= 0.8,
            "Military at capital {} should be strong", capital.name);
    }

    #[test]
    fn hub_has_strong_trade_presence() {
        let galaxy = generate_galaxy(42);
        let hub = galaxy.systems.iter()
            .find(|s| s.infrastructure_level == InfrastructureLevel::Hub)
            .expect("Should have a hub");
        let econ = find_faction_by_category(&galaxy.factions, FactionCategory::Economic);
        let tp = hub.faction_presence.iter().find(|fp| fp.faction_id == econ.id);
        assert!(tp.is_some(), "Hub {} should have economic presence", hub.name);
        assert!(tp.unwrap().strength >= 0.7,
            "Economic at hub {} should be strong", hub.name);
    }

    #[test]
    fn frontier_has_criminal_presence() {
        let galaxy = generate_galaxy(42);
        // Find an outpost-level system.
        let outpost = galaxy.systems.iter()
            .find(|s| s.infrastructure_level == InfrastructureLevel::Outpost)
            .expect("Should have an outpost");
        let frontier = find_criminal_frontier(&galaxy.factions);
        assert!(outpost.faction_presence.iter().any(|fp| fp.faction_id == frontier.id),
            "Outpost {} should have frontier criminal presence", outpost.name);
    }

    #[test]
    fn religious_drawn_to_distorted_space() {
        let galaxy = generate_galaxy(42);
        let order = find_faction_by_category(&galaxy.factions, FactionCategory::Religious);
        // Systems with high time distortion should attract religious presence.
        let distorted: Vec<&StarSystem> = galaxy.systems.iter()
            .filter(|s| s.time_factor >= 1.5)
            .collect();
        assert!(!distorted.is_empty(), "Should have systems with time distortion");
        let has_religious = distorted.iter()
            .any(|s| s.faction_presence.iter().any(|fp| fp.faction_id == order.id));
        assert!(has_religious, "Religious faction should be present in at least one distorted system");

        // Capitals should NOT have religious presence (normal time, no distortion draw).
        // (This might not hold for all seeds since capital could have other draws, so just check
        // that religious presence correlates with distortion.)
    }

    #[test]
    fn covert_criminal_absent_from_wilderness() {
        let galaxy = generate_galaxy(42);
        // Covert criminal is optional — may not exist in every galaxy.
        let covert = galaxy.factions.iter().find(|f| {
            f.category == FactionCategory::Criminal
                && matches!(f.scope, FactionScope::Transnational { .. })
        });
        if let Some(covert) = covert {
            // Wilderness systems (None infra) should have no covert criminal presence.
            for sys in galaxy.systems.iter()
                .filter(|s| s.infrastructure_level == InfrastructureLevel::None)
            {
                assert!(!sys.faction_presence.iter().any(|fp| fp.faction_id == covert.id),
                    "Covert criminal should NOT be in wilderness {}", sys.name);
            }
        }
    }

    #[test]
    fn every_faction_presence_has_services() {
        let galaxy = generate_galaxy(42);
        for sys in &galaxy.systems {
            for fp in &sys.faction_presence {
                assert!(!fp.services.is_empty(), "Presence in {} has no services", sys.name);
            }
        }
    }

    #[test]
    fn faction_scope_civ_ids_reference_valid_civs() {
        let galaxy = generate_galaxy(42);
        let civ_ids: HashSet<Uuid> = galaxy.civilizations.iter().map(|c| c.id).collect();
        for f in &galaxy.factions {
            match &f.scope {
                FactionScope::CivInternal { civ_id } => {
                    assert!(civ_ids.contains(civ_id), "Faction {} CivInternal refs non-existent civ", f.name);
                }
                FactionScope::Transnational { civ_ids: sids } => {
                    for cid in sids {
                        assert!(civ_ids.contains(cid), "Faction {} Transnational refs non-existent civ", f.name);
                    }
                }
                FactionScope::Independent => {}
            }
        }
    }

    #[test]
    fn military_faction_name_contains_civ_prefix() {
        let galaxy = generate_galaxy(42);
        let mil = find_faction_by_category(&galaxy.factions, FactionCategory::Military);
        let civ_prefixes: Vec<&str> = galaxy.civilizations.iter()
            .map(|c| extract_civ_prefix(&c.name)).collect();
        assert!(civ_prefixes.iter().any(|p| mil.name.contains(p)),
            "Military faction '{}' should contain a civ prefix (civs: {:?})", mil.name, civ_prefixes);
    }

    // --- NPC generation ---

    #[test]
    fn npcs_generated_at_colony_plus_systems() {
        let galaxy = generate_galaxy(42);
        assert!(!galaxy.npcs.is_empty(), "Should generate at least some NPCs");

        // Every NPC should be at a Colony+ system.
        for npc in &galaxy.npcs {
            let system = galaxy.systems.iter().find(|s| s.id == npc.home_system_id)
                .expect("NPC home system should exist");
            assert!(
                !matches!(system.infrastructure_level,
                    InfrastructureLevel::None | InfrastructureLevel::Outpost),
                "NPC {} should not be at {:?} system {}",
                npc.name, system.infrastructure_level, system.name,
            );
        }
    }

    #[test]
    fn npcs_have_valid_faction_refs() {
        let galaxy = generate_galaxy(42);
        let faction_ids: HashSet<Uuid> = galaxy.factions.iter().map(|f| f.id).collect();
        for npc in &galaxy.npcs {
            if let Some(fid) = npc.faction_id {
                assert!(faction_ids.contains(&fid),
                    "NPC {} references non-existent faction", npc.name);
            }
        }
    }

    #[test]
    fn capital_has_npcs() {
        let galaxy = generate_galaxy(42);
        let capital = galaxy.systems.iter()
            .find(|s| s.infrastructure_level == InfrastructureLevel::Capital)
            .expect("Should have a capital");
        let capital_npcs: Vec<&Npc> = galaxy.npcs.iter()
            .filter(|n| n.home_system_id == capital.id)
            .collect();
        assert!(!capital_npcs.is_empty(),
            "Capital {} should have at least one NPC", capital.name);
        for npc in &capital_npcs {
            println!("  {} — {}", npc.name, npc.title);
        }
    }

    // --- Economy generation ---

    #[test]
    fn inhabited_systems_have_economies() {
        let galaxy = generate_galaxy(42);
        for system in &galaxy.systems {
            match system.infrastructure_level {
                InfrastructureLevel::None => {
                    assert!(system.economy.is_none(),
                        "{} (None) should not have economy", system.name);
                }
                _ => {
                    assert!(system.economy.is_some(),
                        "{} ({:?}) should have economy",
                        system.name, system.infrastructure_level);
                }
            }
        }
    }

    #[test]
    fn economy_prices_are_reasonable() {
        let galaxy = generate_galaxy(42);
        for system in &galaxy.systems {
            if let Some(ref econ) = system.economy {
                assert!(econ.fuel_price > 0.0 && econ.fuel_price < 20.0,
                    "{} fuel price {:.1} out of range", system.name, econ.fuel_price);
                assert!(econ.supply_price > 0.0 && econ.supply_price < 15.0,
                    "{} supply price {:.1} out of range", system.name, econ.supply_price);
                for good in TradeGood::all() {
                    let buy = econ.buy_price(*good);
                    let sell = econ.sell_price(*good);
                    assert!(buy > 0.0, "{} buy price for {:?} should be positive", system.name, good);
                    assert!(sell < buy, "{} sell price should be less than buy for {:?}", system.name, good);
                }
            }
        }
    }

    #[test]
    fn trade_routes_exist() {
        // At least one pair of systems should have a meaningful price difference
        // on some good, making trade profitable.
        let galaxy = generate_galaxy(42);
        let economies: Vec<(&str, &SystemEconomy)> = galaxy.systems.iter()
            .filter_map(|s| s.economy.as_ref().map(|e| (s.name.as_str(), e)))
            .collect();

        let mut found_route = false;
        for good in TradeGood::all() {
            for (name_a, econ_a) in &economies {
                for (name_b, econ_b) in &economies {
                    if name_a == name_b { continue; }
                    let buy_at_a = econ_a.buy_price(*good);
                    let sell_at_b = econ_b.sell_price(*good);
                    if sell_at_b > buy_at_a {
                        found_route = true;
                    }
                }
            }
        }
        assert!(found_route, "Should have at least one profitable trade route");
    }

    // --- Procedural system generation ---

    #[test]
    fn system_names_are_unique() {
        for seed in [42, 123, 999, 7777] {
            let galaxy = generate_galaxy(seed);
            let mut names: Vec<&str> = galaxy.systems.iter().map(|s| s.name.as_str()).collect();
            let count = names.len();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), count,
                "Seed {} produced duplicate system names", seed);
        }
    }

    #[test]
    fn start_system_is_a_hub() {
        let galaxy = generate_galaxy(42);
        let start = galaxy.systems.iter()
            .find(|s| s.id == galaxy.start_system_id)
            .expect("start_system_id should reference a valid system");
        assert_eq!(start.infrastructure_level, InfrastructureLevel::Hub,
            "Start system should be a hub, got {:?}", start.infrastructure_level);
    }

    #[test]
    fn each_civ_has_a_capital() {
        let galaxy = generate_galaxy(42);
        for civ in &galaxy.civilizations {
            let has_capital = galaxy.systems.iter().any(|s|
                s.controlling_civ == Some(civ.id)
                && s.infrastructure_level == InfrastructureLevel::Capital
            );
            assert!(has_capital, "Civ {} should have a capital system", civ.name);
        }
    }

    #[test]
    fn different_seeds_produce_different_names() {
        let g1 = generate_galaxy(42);
        let g2 = generate_galaxy(999);
        let names1: Vec<&str> = g1.systems.iter().map(|s| s.name.as_str()).collect();
        let names2: Vec<&str> = g2.systems.iter().map(|s| s.name.as_str()).collect();
        assert_ne!(names1, names2, "Different seeds should produce different system names");
    }
}