// file: crates/game/src/crew_conversation.rs
//! Crew conversation system — personality-driven dialogue with reactive topics.
//!
//! Crew conversations are a dedicated system, not encounter pipeline events.
//! They have a different rhythm: you're checking in with someone you know.
//! Topics emerge from who they ARE (personality drives, trust levels) and
//! what's HAPPENING (recent events, ship state, active threads).
//!
//! ## Topic generation
//!
//! 1. **Baseline topics** from personality drives — the crew member's top
//!    drives generate conversation hooks about what matters to them.
//! 2. **Reactive topics** from game state — ship damage, low fuel, recent
//!    encounters, high stress, active threads. These are urgent and can
//!    override baseline topics.
//! 3. **Trust-gated topics** — deeper conversations unlock as trust grows.
//!    Low trust produces guarded exchanges. High trust produces vulnerability.
//!
//! Topics are scored by urgency. The system picks the top 2–3 with some
//! variety (won't pick 3 stress topics). Each topic has 2–3 response
//! options that shift trust axes, stress, mood, and can spawn threads.

use starbound_core::crew::{CrewMember, Mood};
use starbound_core::journey::Journey;
use starbound_core::narrative::{ResolutionState, ThreadType};

// ---------------------------------------------------------------------------
// Topic data model
// ---------------------------------------------------------------------------

/// A conversation topic the crew member might bring up.
#[derive(Debug, Clone)]
pub struct ConversationTopic {
    /// Identifier for anti-repeat tracking.
    pub id: String,
    /// Where this topic came from.
    pub source: TopicSource,
    /// How pressing this is. Higher urgency = more likely to surface.
    /// 0.0–1.0. Reactive topics generally score higher than baseline.
    pub urgency: f32,
    /// What the crew member says or brings up. Written in third person
    /// with their name, suitable for the CLI to display directly.
    pub prompt: String,
    /// How the captain can respond. 2–3 options with different effects.
    pub responses: Vec<ConversationResponse>,
}

/// Where a topic originated.
#[derive(Debug, Clone, PartialEq)]
pub enum TopicSource {
    /// From one of their core personality drives.
    Drive(String),
    /// From their active_concerns list.
    Concern,
    /// From ship/resource state.
    ShipState,
    /// From a recent event in the log.
    RecentEvent,
    /// From an active narrative thread.
    ThreadReaction,
    /// From the trust relationship with the captain.
    TrustDynamic,
    /// From their current stress level.
    StressResponse,
}

/// One way the captain can respond.
#[derive(Debug, Clone)]
pub struct ConversationResponse {
    /// Short label — what the captain does.
    pub label: String,
    /// The effects this response produces.
    pub effects: Vec<ConversationEffect>,
    /// Optional follow-up line from the crew member (for display).
    pub follow_up: Option<String>,
}

/// Atomic conversation effects. Converted to game Effects for application.
#[derive(Debug, Clone)]
pub enum ConversationEffect {
    TrustProfessional(f32),
    TrustPersonal(f32),
    TrustIdeological(f32),
    Stress(f32),
    SetMood(Mood),
    SpawnThread {
        thread_type: ThreadType,
        description: String,
    },
    RemoveConcern(String),
    Narrative(String),
}

// ---------------------------------------------------------------------------
// Topic generation — the main entry point
// ---------------------------------------------------------------------------

/// Generate conversation topics for a crew member, considering both
/// their personality and the current game state.
///
/// Returns topics sorted by urgency (highest first). The caller should
/// present the top 2–3 to the player.
pub fn generate_topics(
    member: &CrewMember,
    journey: &Journey,
    recently_discussed: &[String],
) -> Vec<ConversationTopic> {
    let mut topics = Vec::new();

    // --- Reactive topics (high urgency, conditional) ---
    stress_topics(member, &mut topics);
    ship_state_topics(member, journey, &mut topics);
    thread_reaction_topics(member, journey, &mut topics);
    concern_topics(member, &mut topics);

    // --- Drive-based baseline topics ---
    drive_topics(member, &mut topics);

    // --- Trust-gated topics ---
    trust_topics(member, &mut topics);

    // Filter out recently discussed topics.
    topics.retain(|t| !recently_discussed.contains(&t.id));

    // Sort by urgency descending.
    topics.sort_by(|a, b| b.urgency.partial_cmp(&a.urgency).unwrap());

    // Diversity pass: if top 3 are all the same source type, demote the third.
    if topics.len() >= 3 && topics[0].source == topics[1].source && topics[1].source == topics[2].source {
        if let Some(diverse) = topics[3..].iter().position(|t| t.source != topics[0].source) {
            let idx = diverse + 3;
            let topic = topics.remove(idx);
            topics.insert(2, topic);
        }
    }

    topics
}

// ---------------------------------------------------------------------------
// Reactive topic generators
// ---------------------------------------------------------------------------

fn stress_topics(member: &CrewMember, topics: &mut Vec<ConversationTopic>) {
    if member.state.stress > 0.7 {
        topics.push(ConversationTopic {
            id: "stress_critical".into(),
            source: TopicSource::StressResponse,
            urgency: 0.95,
            prompt: format!(
                "{} looks worn thin. There's a tightness around their eyes that wasn't there \
                 before. When you sit down across from them, it takes a moment before they \
                 look up.\n\n\
                 \"I'm fine. I know that's what everyone says. But I need to say it out loud \
                 or I won't believe it.\"",
                member.name
            ),
            responses: vec![
                ConversationResponse {
                    label: "\"You don't have to be fine.\"".into(),
                    effects: vec![
                        ConversationEffect::Stress(-0.15),
                        ConversationEffect::TrustPersonal(0.1),
                        ConversationEffect::SetMood(Mood::Content),
                        ConversationEffect::Narrative(format!(
                            "Talked {} down from the edge. Sometimes that's the job.", member.name
                        )),
                    ],
                    follow_up: Some("A long exhale. Something loosens.".into()),
                },
                ConversationResponse {
                    label: "\"Tell me what you need.\"".into(),
                    effects: vec![
                        ConversationEffect::Stress(-0.1),
                        ConversationEffect::TrustProfessional(0.08),
                        ConversationEffect::Narrative(format!(
                            "Asked {} what they needed. Got a real answer.", member.name
                        )),
                    ],
                    follow_up: Some("\"Rest. Shore leave. Something that isn't this.\"".into()),
                },
                ConversationResponse {
                    label: "\"I need you functional. What will it take?\"".into(),
                    effects: vec![
                        ConversationEffect::Stress(-0.05),
                        ConversationEffect::TrustProfessional(0.03),
                        ConversationEffect::TrustPersonal(-0.05),
                        ConversationEffect::Narrative(format!(
                            "Addressed {}'s stress. Efficiently.", member.name
                        )),
                    ],
                    follow_up: Some("The professional mask goes back on. It'll hold. For now.".into()),
                },
            ],
        });
    } else if member.state.stress > 0.5 {
        topics.push(ConversationTopic {
            id: "stress_moderate".into(),
            source: TopicSource::StressResponse,
            urgency: 0.6,
            prompt: format!(
                "{} is quieter than usual. Not withdrawn — just... conserving. \
                 You can see them choosing which conversations to have and which to skip.",
                member.name
            ),
            responses: vec![
                ConversationResponse {
                    label: "Ask how they're holding up".into(),
                    effects: vec![
                        ConversationEffect::Stress(-0.08),
                        ConversationEffect::TrustPersonal(0.05),
                    ],
                    follow_up: Some("\"Managing. Some days are easier.\"".into()),
                },
                ConversationResponse {
                    label: "Give them space".into(),
                    effects: vec![
                        ConversationEffect::TrustProfessional(0.02),
                    ],
                    follow_up: None,
                },
            ],
        });
    }
}

fn ship_state_topics(member: &CrewMember, journey: &Journey, topics: &mut Vec<ConversationTopic>) {
    let fuel_frac = journey.ship.fuel / journey.ship.fuel_capacity;
    let supply_frac = journey.ship.supplies / journey.ship.supply_capacity;

    // Hull critical
    if journey.ship.hull_condition < 0.4 && member.drives.security > 0.4 {
        topics.push(ConversationTopic {
            id: "ship_hull_critical".into(),
            source: TopicSource::ShipState,
            urgency: 0.8,
            prompt: format!(
                "{} catches you in the corridor. They've been looking at the hull \
                 diagnostics again.\n\n\
                 \"Captain, I need to be straight with you. We take one more hit \
                 like the last one, and this ship becomes a very expensive coffin.\"",
                member.name
            ),
            responses: vec![
                ConversationResponse {
                    label: "\"I know. Repair is the priority.\"".into(),
                    effects: vec![
                        ConversationEffect::TrustProfessional(0.08),
                        ConversationEffect::Stress(-0.05),
                    ],
                    follow_up: Some("A nod. They needed to hear that.".into()),
                },
                ConversationResponse {
                    label: "\"We'll make it. We always do.\"".into(),
                    effects: vec![
                        ConversationEffect::TrustProfessional(-0.03),
                        ConversationEffect::TrustIdeological(0.03),
                    ],
                    follow_up: Some(
                        "\"Optimism isn't a structural material, Captain.\"".into()
                    ),
                },
            ],
        });
    }

    // Fuel low
    if fuel_frac < 0.25 && member.drives.security > 0.3 {
        topics.push(ConversationTopic {
            id: "fuel_low".into(),
            source: TopicSource::ShipState,
            urgency: 0.75,
            prompt: format!(
                "{} glances at the fuel readout on the bridge display. You both \
                 know the number.\n\n\
                 \"If we don't refuel soon, our options start disappearing.\"",
                member.name
            ),
            responses: vec![
                ConversationResponse {
                    label: "\"I have a plan.\"".into(),
                    effects: vec![
                        ConversationEffect::TrustProfessional(0.05),
                        ConversationEffect::Stress(-0.03),
                    ],
                    follow_up: Some("\"Good. I'd like to hear it sometime.\"".into()),
                },
                ConversationResponse {
                    label: "\"I know. We'll figure it out.\"".into(),
                    effects: vec![
                        ConversationEffect::TrustPersonal(0.02),
                    ],
                    follow_up: Some("Not the reassurance they wanted, but it's honest.".into()),
                },
            ],
        });
    }

    // Supplies low
    if supply_frac < 0.25 {
        topics.push(ConversationTopic {
            id: "supplies_low".into(),
            source: TopicSource::ShipState,
            urgency: 0.7,
            prompt: format!(
                "{} is rationing. You can see it — smaller portions, recycled water, \
                 fewer lights in the off-duty sections.\n\n\
                 \"We need to resupply. Not eventually — soon.\"",
                member.name
            ),
            responses: vec![
                ConversationResponse {
                    label: "\"Next stop. I promise.\"".into(),
                    effects: vec![
                        ConversationEffect::Stress(-0.03),
                        ConversationEffect::TrustProfessional(0.03),
                    ],
                    follow_up: Some("Promises cost nothing to make. They'll remember.".into()),
                },
                ConversationResponse {
                    label: "\"We can stretch it. Your rationing is good work.\"".into(),
                    effects: vec![
                        ConversationEffect::TrustProfessional(0.05),
                        ConversationEffect::SetMood(Mood::Determined),
                    ],
                    follow_up: Some("Professional recognition. It helps.".into()),
                },
            ],
        });
    }
}

fn thread_reaction_topics(
    member: &CrewMember,
    journey: &Journey,
    topics: &mut Vec<ConversationTopic>,
) {
    // Find the most recent open thread.
    let recent_thread = journey
        .threads
        .iter()
        .filter(|t| {
            t.resolution == ResolutionState::Open || t.resolution == ResolutionState::Partial
        })
        .last();

    if let Some(thread) = recent_thread {
        let short_desc: String = thread.description.chars().take(80).collect();

        match thread.thread_type {
            ThreadType::Mystery | ThreadType::Anomaly => {
                if member.drives.knowledge > 0.5 {
                    topics.push(ConversationTopic {
                        id: format!("thread_curiosity_{}", &thread.id.to_string()[..8]),
                        source: TopicSource::ThreadReaction,
                        urgency: 0.55,
                        prompt: format!(
                            "{} has been going over the sensor logs again. You find them \
                             at a terminal, surrounded by graphs and waveforms.\n\n\
                             \"That thing we found — {}. I keep coming back to it. \
                             There's a pattern here.\"",
                            member.name, short_desc
                        ),
                        responses: vec![
                            ConversationResponse {
                                label: "\"What are you seeing?\"".into(),
                                effects: vec![
                                    ConversationEffect::TrustProfessional(0.05),
                                    ConversationEffect::TrustIdeological(0.03),
                                    ConversationEffect::Narrative(format!(
                                        "Discussed the {} findings with {}.",
                                        thread.thread_type, member.name
                                    )),
                                ],
                                follow_up: Some(
                                    "They light up. This is what they're for.".into(),
                                ),
                            },
                            ConversationResponse {
                                label: "\"Focus on your duties first.\"".into(),
                                effects: vec![
                                    ConversationEffect::TrustProfessional(0.02),
                                    ConversationEffect::TrustIdeological(-0.05),
                                ],
                                follow_up: Some(
                                    "The light dims. They nod and close the terminal.".into(),
                                ),
                            },
                        ],
                    });
                }
            }
            ThreadType::Grudge | ThreadType::Debt => {
                if member.drives.justice > 0.5 || member.drives.security > 0.5 {
                    topics.push(ConversationTopic {
                        id: format!("thread_concern_{}", &thread.id.to_string()[..8]),
                        source: TopicSource::ThreadReaction,
                        urgency: 0.6,
                        prompt: format!(
                            "{} brings it up over coffee. Casual tone, careful words.\n\n\
                             \"That situation — {}. It's not resolved. \
                             Are we going to deal with it, or pretend it's behind us?\"",
                            member.name, short_desc
                        ),
                        responses: vec![
                            ConversationResponse {
                                label: "\"We'll deal with it. When the time is right.\"".into(),
                                effects: vec![
                                    ConversationEffect::TrustProfessional(0.05),
                                    ConversationEffect::TrustIdeological(0.03),
                                ],
                                follow_up: Some("\"I'll hold you to that.\"".into()),
                            },
                            ConversationResponse {
                                label: "\"Some things you leave behind.\"".into(),
                                effects: vec![
                                    ConversationEffect::TrustIdeological(-0.05),
                                    ConversationEffect::TrustPersonal(0.02),
                                ],
                                follow_up: Some(
                                    "Disagreement, but they understand the calculus.".into(),
                                ),
                            },
                        ],
                    });
                }
            }
            ThreadType::Relationship => {
                if member.drives.connection > 0.5 {
                    topics.push(ConversationTopic {
                        id: format!("thread_bond_{}", &thread.id.to_string()[..8]),
                        source: TopicSource::ThreadReaction,
                        urgency: 0.4,
                        prompt: format!(
                            "{} is reflective today.\n\n\
                             \"That thing that happened — {}. \
                             I keep thinking about the people involved. \
                             How they'll remember us.\"",
                            member.name, short_desc
                        ),
                        responses: vec![
                            ConversationResponse {
                                label: "\"We did what we could.\"".into(),
                                effects: vec![
                                    ConversationEffect::TrustPersonal(0.05),
                                    ConversationEffect::Stress(-0.03),
                                ],
                                follow_up: Some("\"Maybe. I hope that's enough.\"".into()),
                            },
                            ConversationResponse {
                                label: "Listen in silence".into(),
                                effects: vec![
                                    ConversationEffect::TrustPersonal(0.07),
                                ],
                                follow_up: Some("Sometimes presence is the answer.".into()),
                            },
                        ],
                    });
                }
            }
            _ => {}
        }
    }
}

fn concern_topics(member: &CrewMember, topics: &mut Vec<ConversationTopic>) {
    // Surface their active concerns as conversation starters.
    for concern in &member.state.active_concerns {
        topics.push(ConversationTopic {
            id: format!("concern_{}", concern.chars().take(20).collect::<String>()
                .to_lowercase().replace(' ', "_")),
            source: TopicSource::Concern,
            urgency: 0.5,
            prompt: format!(
                "{} has something on their mind.\n\n\
                 \"I've been thinking about — well. {}. \
                 It's not urgent. But it's there.\"",
                member.name, concern
            ),
            responses: vec![
                ConversationResponse {
                    label: "\"Tell me more.\"".into(),
                    effects: vec![
                        ConversationEffect::TrustPersonal(0.05),
                        ConversationEffect::Stress(-0.03),
                        ConversationEffect::RemoveConcern(concern.clone()),
                        ConversationEffect::Narrative(format!(
                            "Talked through {}'s concern: {}", member.name, concern
                        )),
                    ],
                    follow_up: Some("They talk. You listen. The weight shifts a little.".into()),
                },
                ConversationResponse {
                    label: "\"Noted. We'll address it when we can.\"".into(),
                    effects: vec![
                        ConversationEffect::TrustProfessional(0.02),
                    ],
                    follow_up: Some("Acknowledged but not resolved. It'll come up again.".into()),
                },
            ],
        });
    }
}

// ---------------------------------------------------------------------------
// Drive-based baseline topic generators
// ---------------------------------------------------------------------------

fn drive_topics(member: &CrewMember, topics: &mut Vec<ConversationTopic>) {
    let d = &member.drives;

    // Find their top two drives.
    let mut drives = vec![
        ("security", d.security),
        ("freedom", d.freedom),
        ("purpose", d.purpose),
        ("connection", d.connection),
        ("knowledge", d.knowledge),
        ("justice", d.justice),
    ];
    drives.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    // Generate a topic for each strong drive (> 0.5).
    for &(drive_name, strength) in drives.iter().take(2) {
        if strength < 0.5 {
            continue;
        }

        let topic = match drive_name {
            "security" => security_topic(member),
            "freedom" => freedom_topic(member),
            "purpose" => purpose_topic(member),
            "connection" => connection_topic(member),
            "knowledge" => knowledge_topic(member),
            "justice" => justice_topic(member),
            _ => continue,
        };
        topics.push(topic);
    }
}

fn security_topic(member: &CrewMember) -> ConversationTopic {
    ConversationTopic {
        id: "drive_security".into(),
        source: TopicSource::Drive("security".into()),
        urgency: 0.35,
        prompt: format!(
            "{} is reviewing the ship's readiness report. Again.\n\n\
             \"I've been running scenarios. If something goes wrong out here — \
             and something always goes wrong — are we ready? Honestly?\"",
            member.name
        ),
        responses: vec![
            ConversationResponse {
                label: "Walk through the contingencies together".into(),
                effects: vec![
                    ConversationEffect::TrustProfessional(0.07),
                    ConversationEffect::Stress(-0.03),
                    ConversationEffect::Narrative(format!(
                        "Reviewed emergency protocols with {}.", member.name
                    )),
                ],
                follow_up: Some("Preparation is their love language. This mattered.".into()),
            },
            ConversationResponse {
                label: "\"We'll handle it when it comes.\"".into(),
                effects: vec![
                    ConversationEffect::TrustProfessional(-0.03),
                    ConversationEffect::TrustPersonal(0.02),
                ],
                follow_up: Some(
                    "Not the answer they wanted. But they accept it.".into(),
                ),
            },
        ],
    }
}

fn freedom_topic(member: &CrewMember) -> ConversationTopic {
    ConversationTopic {
        id: "drive_freedom".into(),
        source: TopicSource::Drive("freedom".into()),
        urgency: 0.3,
        prompt: format!(
            "{} is standing at the observation window. Not looking at anything \
             in particular — just looking at the open.\n\n\
             \"You ever think about what comes after? After the mission, after \
             this crew. Where you'd go if you could go anywhere.\"",
            member.name
        ),
        responses: vec![
            ConversationResponse {
                label: "Share your own answer".into(),
                effects: vec![
                    ConversationEffect::TrustPersonal(0.08),
                    ConversationEffect::SetMood(Mood::Hopeful),
                    ConversationEffect::Narrative(format!(
                        "Talked about the future with {}.", member.name
                    )),
                ],
                follow_up: Some("The kind of conversation that makes the ship feel smaller, in a good way.".into()),
            },
            ConversationResponse {
                label: "\"One thing at a time.\"".into(),
                effects: vec![
                    ConversationEffect::TrustProfessional(0.03),
                ],
                follow_up: Some("They nod. The window is still there.".into()),
            },
        ],
    }
}

fn purpose_topic(member: &CrewMember) -> ConversationTopic {
    ConversationTopic {
        id: "drive_purpose".into(),
        source: TopicSource::Drive("purpose".into()),
        urgency: 0.35,
        prompt: format!(
            "{} catches you after a shift change.\n\n\
             \"I want to make sure I'm pulling my weight. Not just doing my job — \
             actually contributing to what we're out here for. \
             Am I?\"",
            member.name
        ),
        responses: vec![
            ConversationResponse {
                label: "\"This crew doesn't work without you.\"".into(),
                effects: vec![
                    ConversationEffect::TrustProfessional(0.08),
                    ConversationEffect::TrustPersonal(0.03),
                    ConversationEffect::Stress(-0.05),
                    ConversationEffect::SetMood(Mood::Determined),
                ],
                follow_up: Some("It lands. You can see them stand a little straighter.".into()),
            },
            ConversationResponse {
                label: "\"There's room to grow. Let's talk about where.\"".into(),
                effects: vec![
                    ConversationEffect::TrustProfessional(0.1),
                    ConversationEffect::TrustIdeological(0.03),
                ],
                follow_up: Some("Honest feedback. They respect that more than empty praise.".into()),
            },
            ConversationResponse {
                label: "\"We all have doubts. Don't let them drive.\"".into(),
                effects: vec![
                    ConversationEffect::TrustPersonal(0.03),
                    ConversationEffect::Stress(-0.02),
                ],
                follow_up: Some("Generic. True. Not what they needed.".into()),
            },
        ],
    }
}

fn connection_topic(member: &CrewMember) -> ConversationTopic {
    ConversationTopic {
        id: "drive_connection".into(),
        source: TopicSource::Drive("connection".into()),
        urgency: 0.3,
        prompt: format!(
            "You find {} in the mess, holding a cup of something cold. They've been \
             staring at a spot on the table.\n\n\
             \"Do you think they remember us? The people we left behind. \
             Or did we just... fade?\"",
            member.name
        ),
        responses: vec![
            ConversationResponse {
                label: "\"Some people carry you with them. You're hard to forget.\"".into(),
                effects: vec![
                    ConversationEffect::TrustPersonal(0.1),
                    ConversationEffect::Stress(-0.05),
                    ConversationEffect::SetMood(Mood::Hopeful),
                ],
                follow_up: Some("A small smile. The cold cup gets refilled.".into()),
            },
            ConversationResponse {
                label: "\"We're building something here. This crew is real.\"".into(),
                effects: vec![
                    ConversationEffect::TrustPersonal(0.05),
                    ConversationEffect::TrustIdeological(0.05),
                    ConversationEffect::SpawnThread {
                        thread_type: ThreadType::Relationship,
                        description: format!(
                            "{} opened up about missing home. The walls thinned.",
                            member.name
                        ),
                    },
                ],
                follow_up: Some("They look at you differently after that.".into()),
            },
            ConversationResponse {
                label: "\"I don't know. I try not to think about it.\"".into(),
                effects: vec![
                    ConversationEffect::TrustPersonal(0.03),
                ],
                follow_up: Some("Honest. Sometimes that's enough.".into()),
            },
        ],
    }
}

fn knowledge_topic(member: &CrewMember) -> ConversationTopic {
    ConversationTopic {
        id: "drive_knowledge".into(),
        source: TopicSource::Drive("knowledge".into()),
        urgency: 0.3,
        prompt: format!(
            "{} has been reading again — data archives, old survey reports, anything \
             they can get from the ship's library.\n\n\
             \"There are gaps in the records out here. Whole systems that were surveyed \
             once, decades ago, and never revisited. Doesn't that bother you?\"",
            member.name
        ),
        responses: vec![
            ConversationResponse {
                label: "\"That's why we're out here.\"".into(),
                effects: vec![
                    ConversationEffect::TrustIdeological(0.08),
                    ConversationEffect::SetMood(Mood::Inspired),
                ],
                follow_up: Some("Exactly what they needed to hear.".into()),
            },
            ConversationResponse {
                label: "\"Gaps in records usually mean someone wanted them gone.\"".into(),
                effects: vec![
                    ConversationEffect::TrustProfessional(0.05),
                    ConversationEffect::SpawnThread {
                        thread_type: ThreadType::Mystery,
                        description: format!(
                            "{} noticed gaps in the survey records. Deliberate omissions?",
                            member.name
                        ),
                    },
                ],
                follow_up: Some("Their eyes widen slightly. That thought hadn't occurred to them.".into()),
            },
        ],
    }
}

fn justice_topic(member: &CrewMember) -> ConversationTopic {
    ConversationTopic {
        id: "drive_justice".into(),
        source: TopicSource::Drive("justice".into()),
        urgency: 0.35,
        prompt: format!(
            "{} has been quiet since the last port. Something is working on them.\n\n\
             \"The people at that last station. The way they looked at us — like we \
             were just passing through their disaster. And we were. We passed through.\"",
            member.name
        ),
        responses: vec![
            ConversationResponse {
                label: "\"We can't save everyone. But we can choose who we help next.\"".into(),
                effects: vec![
                    ConversationEffect::TrustIdeological(0.08),
                    ConversationEffect::TrustPersonal(0.03),
                    ConversationEffect::Stress(-0.03),
                ],
                follow_up: Some("A long breath. \"Yeah. Okay. Next time.\"".into()),
            },
            ConversationResponse {
                label: "\"We had our own problems. You can't carry all of it.\"".into(),
                effects: vec![
                    ConversationEffect::TrustIdeological(-0.05),
                    ConversationEffect::TrustPersonal(0.02),
                ],
                follow_up: Some("They disagree. But they understand the weight.".into()),
            },
            ConversationResponse {
                label: "\"What would you have done differently?\"".into(),
                effects: vec![
                    ConversationEffect::TrustProfessional(0.05),
                    ConversationEffect::TrustIdeological(0.05),
                ],
                follow_up: Some("They have an answer. It's a good one.".into()),
            },
        ],
    }
}

// ---------------------------------------------------------------------------
// Trust-gated topics
// ---------------------------------------------------------------------------

fn trust_topics(member: &CrewMember, topics: &mut Vec<ConversationTopic>) {
    let t = &member.trust;

    // Low professional trust — they're questioning your competence.
    if t.professional < 0.15 {
        topics.push(ConversationTopic {
            id: "trust_low_professional".into(),
            source: TopicSource::TrustDynamic,
            urgency: 0.65,
            prompt: format!(
                "{} has been professional but distant. Correct in every interaction \
                 but not an ounce more.\n\n\
                 They look at you directly. \"Permission to speak freely?\"",
                member.name
            ),
            responses: vec![
                ConversationResponse {
                    label: "\"Always.\"".into(),
                    effects: vec![
                        ConversationEffect::TrustProfessional(0.05),
                        ConversationEffect::TrustPersonal(0.03),
                        ConversationEffect::Narrative(format!(
                            "{} aired concerns about leadership. Received openly.", member.name
                        )),
                    ],
                    follow_up: Some(
                        "What follows is honest. Hard to hear. Necessary.".into(),
                    ),
                },
                ConversationResponse {
                    label: "\"Noted. But I need results, not opinions.\"".into(),
                    effects: vec![
                        ConversationEffect::TrustProfessional(-0.03),
                        ConversationEffect::TrustPersonal(-0.05),
                        ConversationEffect::SetMood(Mood::Withdrawn),
                    ],
                    follow_up: Some("The mask goes on. It won't come off again soon.".into()),
                },
            ],
        });
    }

    // High personal trust — vulnerability unlocks.
    if t.personal > 0.5 {
        topics.push(ConversationTopic {
            id: "trust_high_personal".into(),
            source: TopicSource::TrustDynamic,
            urgency: 0.25,
            prompt: format!(
                "Late shift. {} sits down next to you in the observation lounge \
                 without asking permission. That's new.\n\n\
                 \"Can I tell you something I haven't told anyone on this ship?\"",
                member.name
            ),
            responses: vec![
                ConversationResponse {
                    label: "Listen".into(),
                    effects: vec![
                        ConversationEffect::TrustPersonal(0.1),
                        ConversationEffect::SpawnThread {
                            thread_type: ThreadType::Relationship,
                            description: format!(
                                "{} shared something deeply personal. A secret held for years.",
                                member.name
                            ),
                        },
                        ConversationEffect::Narrative(format!(
                            "{} trusted you with something real.", member.name
                        )),
                    ],
                    follow_up: Some(
                        "What they tell you is small and enormous at the same time.".into(),
                    ),
                },
                ConversationResponse {
                    label: "\"You don't have to. But I'm here.\"".into(),
                    effects: vec![
                        ConversationEffect::TrustPersonal(0.07),
                        ConversationEffect::SetMood(Mood::Content),
                    ],
                    follow_up: Some("They tell you anyway. Some things need saying.".into()),
                },
            ],
        });
    }
}

// ---------------------------------------------------------------------------
// Effect conversion — translate to game Effects for application
// ---------------------------------------------------------------------------

use crate::consequences::Effect;

/// Convert conversation effects to game effects for apply_effects().
pub fn conversation_effects_to_game_effects(
    effects: &[ConversationEffect],
) -> Vec<Effect> {
    effects
        .iter()
        .map(|e| match e {
            ConversationEffect::TrustProfessional(d) => Effect::TrustProfessional(*d),
            ConversationEffect::TrustPersonal(d) => Effect::TrustPersonal(*d),
            ConversationEffect::TrustIdeological(d) => Effect::TrustIdeological(*d),
            ConversationEffect::Stress(d) => Effect::CrewStress(*d),
            ConversationEffect::SetMood(m) => Effect::CrewMood {
                mood: *m,
                all: false,
            },
            ConversationEffect::SpawnThread {
                thread_type,
                description,
            } => Effect::SpawnThread {
                thread_type: *thread_type,
                description: description.clone(),
            },
            ConversationEffect::RemoveConcern(_) => Effect::Pass,
            ConversationEffect::Narrative(t) => Effect::Narrative(t.clone()),
        })
        .collect()
}

/// Apply the RemoveConcern effects that can't go through the normal effect system.
/// Call this after apply_effects() for conversation responses.
pub fn apply_concern_removals(member: &mut CrewMember, effects: &[ConversationEffect]) {
    for effect in effects {
        if let ConversationEffect::RemoveConcern(concern) = effect {
            member
                .state
                .active_concerns
                .retain(|c| c != concern);
        }
    }
}

// ---------------------------------------------------------------------------
// Narrative state description — show crew state as prose, not numbers
// ---------------------------------------------------------------------------

/// Generate a narrative description of a crew member's current state.
/// Used at the top of the conversation screen instead of raw stats.
pub fn describe_crew_state(member: &CrewMember) -> String {
    let mut lines = Vec::new();

    // Role and mood
    let mood_desc = match member.state.mood {
        Mood::Content => "seems settled",
        Mood::Anxious => "has a tension they can't quite hide",
        Mood::Determined => "carries a quiet intensity",
        Mood::Grieving => "is carrying something heavy",
        Mood::Restless => "can't seem to stay still",
        Mood::Hopeful => "has a lightness about them",
        Mood::Withdrawn => "has pulled inward, away from the crew",
        Mood::Angry => "is running hot, barely contained",
        Mood::Inspired => "is lit up, engaged with everything",
    };
    lines.push(format!("{}, your {}. {}", member.name, member.role, mood_desc));

    // Stress
    if member.state.stress > 0.7 {
        lines.push("They're near the edge — you can see it.".into());
    } else if member.state.stress > 0.4 {
        lines.push("The strain shows in small ways.".into());
    }

    // Trust summary
    let t = &member.trust;
    if t.professional > 0.5 && t.personal > 0.4 {
        lines.push("They trust your judgment and your character.".into());
    } else if t.professional > 0.4 && t.personal < 0.2 {
        lines.push("They respect your competence but keep their distance.".into());
    } else if t.professional < 0.2 {
        lines.push("There's doubt in how they look at you.".into());
    }

    lines.join(" ")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use starbound_core::crew::*;
    use starbound_core::mission::*;
    use starbound_core::reputation::PlayerProfile;
    use starbound_core::ship::*;
    use starbound_core::time::Timestamp;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn test_crew_member(name: &str, role: CrewRole, drives: PersonalityDrives) -> CrewMember {
        CrewMember {
            id: Uuid::new_v4(),
            name: name.into(),
            role,
            drives,
            trust: Trust::starting_crew(),
            relationships: HashMap::new(),
            background: "Test background.".into(),
            state: CrewState {
                mood: Mood::Content,
                stress: 0.2,
                active_concerns: vec![],
            },
            origin: CrewOrigin::Starting,
        }
    }

    fn test_journey() -> Journey {
        Journey {
            ship: Ship {
                name: "Test Ship".into(),
                hull_condition: 0.8,
                fuel: 50.0,
                fuel_capacity: 100.0,
                supplies: 80.0,
                supply_capacity: 100.0,
                cargo: HashMap::new(),
                cargo_capacity: 50,
                modules: ShipModules {
                    engine: Module::standard("Engine"),
                    sensors: Module::standard("Sensors"),
                    comms: Module::standard("Comms"),
                    weapons: Module::standard("Weapons"),
                    life_support: Module::standard("Life Support"),
                },
            },
            current_system: Uuid::new_v4(),
            time: Timestamp::zero(),
            resources: 1000.0,
            mission: MissionState {
                mission_type: MissionType::Search,
                core_truth: "Test".into(),
                knowledge_nodes: vec![],
            },
            crew: vec![],
            threads: vec![],
            event_log: vec![],
            civ_standings: HashMap::new(),
            profile: PlayerProfile::new(),
            active_contracts: vec![],
            discovered_rumors: vec![],
            current_location: None,
        }
    }

    #[test]
    fn high_knowledge_drive_generates_topic() {
        let member = test_crew_member(
            "Kael",
            CrewRole::Navigator,
            PersonalityDrives {
                security: 0.3, freedom: 0.3, purpose: 0.3,
                connection: 0.3, knowledge: 0.8, justice: 0.3,
            },
        );
        let journey = test_journey();
        let topics = generate_topics(&member, &journey, &[]);
        assert!(
            topics.iter().any(|t| t.id == "drive_knowledge"),
            "High knowledge drive should generate knowledge topic"
        );
    }

    #[test]
    fn high_stress_generates_urgent_topic() {
        let mut member = test_crew_member(
            "Reva",
            CrewRole::Engineer,
            PersonalityDrives {
                security: 0.6, freedom: 0.3, purpose: 0.7,
                connection: 0.5, knowledge: 0.3, justice: 0.3,
            },
        );
        member.state.stress = 0.8;
        let journey = test_journey();
        let topics = generate_topics(&member, &journey, &[]);
        assert!(!topics.is_empty());
        assert_eq!(
            topics[0].id, "stress_critical",
            "Critical stress should be the top topic"
        );
    }

    #[test]
    fn low_hull_triggers_ship_state_topic() {
        let member = test_crew_member(
            "Reva",
            CrewRole::Engineer,
            PersonalityDrives {
                security: 0.7, freedom: 0.3, purpose: 0.5,
                connection: 0.3, knowledge: 0.3, justice: 0.3,
            },
        );
        let mut journey = test_journey();
        journey.ship.hull_condition = 0.3;
        let topics = generate_topics(&member, &journey, &[]);
        assert!(
            topics.iter().any(|t| t.id == "ship_hull_critical"),
            "Low hull + security drive should generate hull topic"
        );
    }

    #[test]
    fn recently_discussed_topics_are_filtered() {
        let member = test_crew_member(
            "Kael",
            CrewRole::Navigator,
            PersonalityDrives {
                security: 0.3, freedom: 0.3, purpose: 0.3,
                connection: 0.3, knowledge: 0.8, justice: 0.3,
            },
        );
        let journey = test_journey();
        let discussed = vec!["drive_knowledge".to_string()];
        let topics = generate_topics(&member, &journey, &discussed);
        assert!(
            !topics.iter().any(|t| t.id == "drive_knowledge"),
            "Recently discussed topic should be filtered out"
        );
    }

    #[test]
    fn concerns_generate_topics() {
        let mut member = test_crew_member(
            "Josen",
            CrewRole::Comms,
            PersonalityDrives {
                security: 0.3, freedom: 0.3, purpose: 0.3,
                connection: 0.5, knowledge: 0.5, justice: 0.5,
            },
        );
        member.state.active_concerns.push("Long-range comms static".into());
        let journey = test_journey();
        let topics = generate_topics(&member, &journey, &[]);
        assert!(
            topics.iter().any(|t| t.source == TopicSource::Concern),
            "Active concern should generate a topic"
        );
    }

    #[test]
    fn effect_conversion_works() {
        let effects = vec![
            ConversationEffect::TrustProfessional(0.1),
            ConversationEffect::Stress(-0.05),
            ConversationEffect::Narrative("Talked.".into()),
        ];
        let game_effects = conversation_effects_to_game_effects(&effects);
        assert_eq!(game_effects.len(), 3);
    }

    #[test]
    fn narrative_state_produces_prose() {
        let member = test_crew_member(
            "Kael",
            CrewRole::Navigator,
            PersonalityDrives {
                security: 0.3, freedom: 0.7, purpose: 0.5,
                connection: 0.4, knowledge: 0.8, justice: 0.3,
            },
        );
        let desc = describe_crew_state(&member);
        assert!(desc.contains("Kael"));
        assert!(desc.contains("navigator"));
    }

    #[test]
    fn high_personal_trust_unlocks_vulnerability() {
        let mut member = test_crew_member(
            "Reva",
            CrewRole::Engineer,
            PersonalityDrives {
                security: 0.5, freedom: 0.3, purpose: 0.5,
                connection: 0.5, knowledge: 0.3, justice: 0.3,
            },
        );
        member.trust.personal = 0.6;
        let journey = test_journey();
        let topics = generate_topics(&member, &journey, &[]);
        assert!(
            topics.iter().any(|t| t.id == "trust_high_personal"),
            "High personal trust should unlock vulnerability topic"
        );
    }
}