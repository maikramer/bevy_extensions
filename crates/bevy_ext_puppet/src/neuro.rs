//! Neuromuscular physiology: activation rise/fall dynamics, short-term stagger
//! fatigue, hit-combo trauma accumulation, wound memory (for `ReachForWound`
//! and `BodyWrithe`), gaze coordination, and impact event debouncing.

use bevy::prelude::*;

use crate::part::{PART_COUNT, PuppetPart};

/// Recorded anatomical impact point for pain clutching (`ReachForWound`) and
/// ground restlessness (`BodyWrithe`).
#[derive(Clone, Copy, Debug)]
pub struct WoundMemory {
    pub part: PuppetPart,
    /// Impact point expressed in the struck body's local frame (tracks the body as it moves).
    pub local_point: Vec3,
    /// Unit world direction the blow pushed.
    pub direction: Vec3,
    /// Severity in `[0, 1]`.
    pub severity: f32,
    /// Simulation timestamp (`sense.time`) when received.
    pub time: f32,
}

/// Active gaze target for vestibulocollic head-neck coordination.
#[derive(Clone, Copy, Debug)]
pub struct GazeTarget {
    /// World point the puppet wants to look toward.
    pub point: Vec3,
    /// Priority (higher wins when multiple behaviours set gaze).
    pub priority: i32,
    /// Simulation time (`sense.time`) when this gaze expires.
    pub expires_at: f32,
}

/// Per-puppet neuromuscular state updated every physics step.
#[derive(Component, Clone, Debug)]
pub struct PuppetNeuroState {
    /// Filtered muscle activation `[0, 1]` per joint (models finite neuromuscular
    /// rise time and rapid reflexive inhibition on impact).
    pub activation: [f32; PART_COUNT],
    /// Short-term balance/stagger fatigue `[0, 1]`: builds over prolonged staggers
    /// and heavy impacts, slightly softening balance precision until calm.
    pub fatigue: f32,
    /// Accumulated hit/slam trauma `[0, 1]`: allows consecutive blows to compound
    /// organically from flinch → stagger → knockdown.
    pub trauma: f32,
    /// Number of consecutive hits within the combo decay window.
    pub hit_combo: u32,
    /// Timestamp (`sense.time`) of the most recent hit or heavy bump.
    pub last_hit_time: f32,
    /// Most recent significant wound for `ReachForWound` and `BodyWrithe`.
    pub wound: Option<WoundMemory>,
    /// Active look-at target (bumper, threat, wall, landing zone).
    pub gaze: Option<GazeTarget>,
    /// Cooldown timers (s) per part preventing duplicate `BodyImpact` events while sliding.
    pub impact_cooldown: [f32; PART_COUNT],
}

impl Default for PuppetNeuroState {
    fn default() -> Self {
        Self {
            activation: [1.0; PART_COUNT],
            fatigue: 0.0,
            trauma: 0.0,
            hit_combo: 0,
            last_hit_time: -10.0,
            wound: None,
            gaze: None,
            impact_cooldown: [0.0; PART_COUNT],
        }
    }
}

impl PuppetNeuroState {
    /// Records an incoming hit, compounding `trauma` and updating `wound` memory.
    /// Returns the effective stacked trauma after this hit.
    pub fn record_hit(
        &mut self,
        part: PuppetPart,
        local_point: Vec3,
        direction: Vec3,
        severity: f32,
        now: f32,
    ) -> f32 {
        let sev = severity.clamp(0.0, 1.0);
        if now - self.last_hit_time <= 1.6 {
            self.hit_combo = self.hit_combo.saturating_add(1);
        } else {
            self.hit_combo = 1;
        }
        self.last_hit_time = now;
        // Rapid consecutive hits compound trauma nonlinearly.
        let combo_bonus = 1.0 + 0.18 * (self.hit_combo.saturating_sub(1) as f32).min(4.0);
        self.trauma = (self.trauma + sev * 0.48 * combo_bonus).clamp(0.0, 1.0);
        self.fatigue = (self.fatigue + sev * 0.18).clamp(0.0, 1.0);

        if sev >= 0.18 {
            self.wound = Some(WoundMemory {
                part,
                local_point,
                direction: direction.normalize_or_zero(),
                severity: sev,
                time: now,
            });
        }
        self.trauma
    }

    /// Sets or upgrades the active gaze target if it has >= priority (or the current expired).
    pub fn set_gaze(&mut self, point: Vec3, priority: i32, duration: f32, now: f32) {
        if !point.is_finite() || duration <= 0.0 {
            return;
        }
        let replace = match self.gaze {
            None => true,
            Some(cur) => now >= cur.expires_at || priority >= cur.priority,
        };
        if replace {
            self.gaze = Some(GazeTarget {
                point,
                priority,
                expires_at: now + duration,
            });
        }
    }

    /// Active gaze point if not expired.
    pub fn active_gaze(&self, now: f32) -> Option<Vec3> {
        self.gaze.filter(|g| now < g.expires_at).map(|g| g.point)
    }

    /// Advances decay timers for trauma, fatigue, wound memory, gaze, and impact debouncing.
    pub fn tick(&mut self, dt: f32, now: f32, is_calm: bool) {
        let trauma_tau = if is_calm { 1.4 } else { 2.6 };
        let fatigue_tau = if is_calm { 2.2 } else { 6.0 };
        self.trauma *= (-dt / trauma_tau).exp();
        if self.trauma < 1e-3 {
            self.trauma = 0.0;
            if now - self.last_hit_time > 1.6 {
                self.hit_combo = 0;
            }
        }
        self.fatigue *= (-dt / fatigue_tau).exp();
        if self.fatigue < 1e-3 {
            self.fatigue = 0.0;
        }
        if let Some(w) = self.wound
            && now - w.time > 6.0
        {
            self.wound = None;
        }
        if let Some(g) = self.gaze
            && now >= g.expires_at
        {
            self.gaze = None;
        }
        for cd in &mut self.impact_cooldown {
            *cd = (*cd - dt).max(0.0);
        }
    }

    /// Steps first-order neuromuscular activation dynamics for joint `i`.
    /// Inhibition (dropping strength on hit/buckle) is fast (~20 ms);
    /// recruitment (ramping back up) follows `rise_time` (~45 ms).
    #[inline]
    pub fn step_activation(&mut self, i: usize, target: f32, dt: f32, rise_time: f32, instant: bool) -> f32 {
        let goal = target.clamp(0.0, 2.0);
        if instant || rise_time <= 1e-4 {
            self.activation[i] = goal;
            return goal;
        }
        let cur = self.activation[i];
        let tau = if goal < cur { 0.020 } else { rise_time };
        let alpha = 1.0 - (-dt / tau).exp();
        let next = cur + (goal - cur) * alpha;
        self.activation[i] = next;
        next
    }
}
