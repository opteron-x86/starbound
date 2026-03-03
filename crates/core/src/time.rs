use serde::{Deserialize, Serialize};

/// A point in time measured on both timescales simultaneously.
///
/// Every event in the game is stamped with both personal and galactic time.
/// The divergence between them IS the story — months for the crew,
/// decades for the galaxy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Timestamp {
    /// Days elapsed from the crew's subjective perspective.
    pub personal_days: f64,
    /// Days elapsed in the galaxy at large.
    pub galactic_days: f64,
}

impl Timestamp {
    pub fn zero() -> Self {
        Self {
            personal_days: 0.0,
            galactic_days: 0.0,
        }
    }

    /// How many galactic years have passed per personal year.
    /// A ratio of 1.0 means in-sync. Higher means drifting.
    pub fn dilation_ratio(&self) -> f64 {
        if self.personal_days <= 0.0 {
            return 1.0;
        }
        self.galactic_days / self.personal_days
    }

    /// Galactic time expressed in years (for readability).
    pub fn galactic_years(&self) -> f64 {
        self.galactic_days / 365.25
    }

    /// Personal time expressed in years.
    pub fn personal_years(&self) -> f64 {
        self.personal_days / 365.25
    }
}

impl std::ops::Add<Duration> for Timestamp {
    type Output = Timestamp;
    fn add(self, d: Duration) -> Timestamp {
        Timestamp {
            personal_days: self.personal_days + d.personal_days,
            galactic_days: self.galactic_days + d.galactic_days,
        }
    }
}

impl std::ops::AddAssign<Duration> for Timestamp {
    fn add_assign(&mut self, d: Duration) {
        self.personal_days += d.personal_days;
        self.galactic_days += d.galactic_days;
    }
}

/// A span of time on both scales — used for travel durations and intervals.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Duration {
    pub personal_days: f64,
    pub galactic_days: f64,
}

impl Duration {
    pub fn personal_months(&self) -> f64 {
        self.personal_days / 30.44
    }

    pub fn galactic_years(&self) -> f64 {
        self.galactic_days / 365.25
    }
}
