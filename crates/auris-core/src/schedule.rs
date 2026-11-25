//! Minuterie de sommeil et réveil (équivalent des réglages « Sleep » de Volumio).
//!
//! Logique pure, testable sans horloge réelle : tout passe par `now` (timestamp
//! unix) et `offset_secs` (décalage heure locale vs UTC, ex. 3600 en hiver à Paris).
//!
//! - Sommeil : one-shot. Quand `sleep_timer` vaut N minutes, un délai est armé à
//!   `now + N*60` ; à l'échéance, l'événement `Alarm::Sleep` est émis une fois.
//!   Re-armer la même valeur ne redéclenche rien (il faut changer la valeur ou
//!   la désactiver puis la réactiver).
//! - Réveil : `wake_time` « HH:MM » en heure locale ; à chaque passage de l'heure
//!   cible (une fois par jour), `Alarm::Wake` est émis.

use serde::{Deserialize, Serialize};

use crate::settings::parse_hhmm;

/// Événement d'alarme déclenché par [`tick`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Alarm {
    /// La minuterie de sommeil a expiré : mettre la lecture en pause.
    Sleep,
    /// L'heure de réveil est atteinte : relancer la lecture.
    Wake,
}

/// État interne persistant entre deux appels de [`tick`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlarmState {
    /// Valeur de `sleep_timer` pour laquelle le délai a été armé.
    sleep_setting: Option<u32>,
    /// Échéance du sommeil (timestamp unix), si armée.
    sleep_deadline: Option<i64>,
    /// Le sommeil a déjà fired pour ce réglage (one-shot).
    sleep_fired: bool,
    /// Jour (numéro de jour local) du dernier déclenchement du réveil.
    last_wake_day: Option<i64>,
}

impl AlarmState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Échéance courante de la minuterie de sommeil (si armée et non consommée).
    pub fn sleep_deadline(&self) -> Option<i64> {
        if self.sleep_fired {
            None
        } else {
            self.sleep_deadline
        }
    }
}

/// Secondes écoulées depuis minuit en heure locale.
pub fn local_secs_of_day(now: i64, offset_secs: i32) -> u32 {
    let local = now + i64::from(offset_secs);
    (local.rem_euclid(86_400)) as u32
}

/// Numéro de jour en heure locale (pour le one-shot quotidien du réveil).
pub fn local_day(now: i64, offset_secs: i32) -> i64 {
    (now + i64::from(offset_secs)).div_euclid(86_400)
}

/// Calcule les alarmes à déclencher à l'instant `now`.
///
/// - `sleep_minutes` : réglage courant `playback.sleep_timer` (None = off).
/// - `wake` : réglage courant `playback.wake_time` (« HH:MM » ou None).
/// - `st` : état mis à jour en place.
///
/// Retourne la liste des événements déclenchés (0, 1 ou 2 éléments).
pub fn tick(
    now: i64,
    offset_secs: i32,
    sleep_minutes: Option<u32>,
    wake: Option<&str>,
    st: &mut AlarmState,
) -> Vec<Alarm> {
    let mut events = Vec::new();

    // --- minuterie de sommeil (one-shot) ---
    if st.sleep_setting != sleep_minutes {
        st.sleep_setting = sleep_minutes;
        st.sleep_deadline = sleep_minutes.map(|m| now + i64::from(m) * 60);
        st.sleep_fired = false;
    }
    if let Some(deadline) = st.sleep_deadline {
        if !st.sleep_fired && now >= deadline {
            st.sleep_fired = true;
            events.push(Alarm::Sleep);
        }
    }

    // --- réveil quotidien à HH:MM heure locale ---
    if let Some(target) = wake.and_then(parse_hhmm) {
        let secs = u32::from(target.0) * 3600 + u32::from(target.1) * 60;
        let today = local_day(now, offset_secs);
        // timestamp unix de l'occurrence d'aujourd'hui (heure locale)
        let occurrence = today * 86_400 - i64::from(offset_secs) + i64::from(secs);
        if now >= occurrence && st.last_wake_day != Some(today) {
            st.last_wake_day = Some(today);
            events.push(Alarm::Wake);
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-01-15 12:00:00 UTC = 1768478400
    const NOON: i64 = 1_768_478_400;

    #[test]
    fn local_secs_of_day_et_jour() {
        // UTC
        assert_eq!(local_secs_of_day(NOON, 0), 12 * 3600);
        assert_eq!(local_day(NOON, 0), NOON / 86_400);
        // Paris hiver (UTC+1) : 13:00 locale
        assert_eq!(local_secs_of_day(NOON, 3600), 13 * 3600);
        // après minuit UTC mais encore la veille en heure locale négative (UTC-5)
        let minuit_utc = NOON - 12 * 3600 + 60; // 00:01 UTC
        assert_eq!(local_secs_of_day(minuit_utc, -5 * 3600), 19 * 3600 + 60);
        assert_eq!(local_day(minuit_utc, -5 * 3600), local_day(NOON, 0) - 1);
    }

    #[test]
    fn sleep_se_declenche_une_fois() {
        let mut st = AlarmState::new();
        // armé à t0 pour 30 min
        let ev = tick(NOON, 0, Some(30), None, &mut st);
        assert!(ev.is_empty());
        assert_eq!(st.sleep_deadline(), Some(NOON + 30 * 60));
        // avant l'échéance : rien
        assert!(tick(NOON + 29 * 60, 0, Some(30), None, &mut st).is_empty());
        // à l'échéance : Sleep
        let ev = tick(NOON + 30 * 60, 0, Some(30), None, &mut st);
        assert_eq!(ev, vec![Alarm::Sleep]);
        // one-shot : plus rien ensuite, même bien plus tard
        assert!(tick(NOON + 3600, 0, Some(30), None, &mut st).is_empty());
        assert_eq!(st.sleep_deadline(), None);
        // changer la valeur réarme
        let ev = tick(NOON + 3600, 0, Some(15), None, &mut st);
        assert!(ev.is_empty());
        assert_eq!(st.sleep_deadline(), Some(NOON + 3600 + 15 * 60));
        // désactiver coupe tout
        assert!(tick(NOON + 7200, 0, None, None, &mut st).is_empty());
        assert_eq!(st.sleep_deadline(), None);
    }

    #[test]
    fn sleep_rearm_apres_desactivation() {
        let mut st = AlarmState::new();
        tick(NOON, 0, Some(30), None, &mut st);
        tick(NOON + 30 * 60, 0, Some(30), None, &mut st); // fired
        tick(NOON + 31 * 60, 0, None, None, &mut st); // off
        tick(NOON + 32 * 60, 0, Some(30), None, &mut st); // réarmé
        assert_eq!(st.sleep_deadline(), Some(NOON + 32 * 60 + 30 * 60));
    }

    #[test]
    fn wake_se_declenche_une_fois_par_jour() {
        let mut st = AlarmState::new();
        // réveil à 07:00 UTC ; NOON = 12:00 UTC -> déjà passé aujourd'hui
        let ev = tick(NOON, 0, None, Some("07:00"), &mut st);
        assert_eq!(ev, vec![Alarm::Wake]);
        // plus de second déclenchement le même jour
        assert!(tick(NOON + 3600, 0, None, Some("07:00"), &mut st).is_empty());
        // le lendemain à 07:00 : déclenchement
        let next_day_7h = (NOON / 86_400 + 1) * 86_400 + 7 * 3600;
        assert!(tick(next_day_7h - 60, 0, None, Some("07:00"), &mut st).is_empty());
        let ev = tick(next_day_7h, 0, None, Some("07:00"), &mut st);
        assert_eq!(ev, vec![Alarm::Wake]);
    }

    #[test]
    fn wake_avant_l_heure_ne_declenche_pas() {
        let mut st = AlarmState::new();
        // 06:00 UTC, réveil à 07:00 -> rien
        let t = (NOON / 86_400) * 86_400 + 6 * 3600;
        assert!(tick(t, 0, None, Some("07:00"), &mut st).is_empty());
        // à 07:00 pile : déclenchement
        let ev = tick(t + 3600, 0, None, Some("07:00"), &mut st);
        assert_eq!(ev, vec![Alarm::Wake]);
    }

    #[test]
    fn wake_heure_locale_avec_offset() {
        let mut st = AlarmState::new();
        // 06:30 UTC = 07:30 à Paris (UTC+1) -> réveil "07:00" locale déjà passé
        let t = (NOON / 86_400) * 86_400 + 6 * 3600 + 1800;
        let ev = tick(t, 3600, None, Some("07:00"), &mut st);
        assert_eq!(ev, vec![Alarm::Wake]);
        // à 05:30 UTC = 06:30 locale -> pas encore
        let mut st2 = AlarmState::new();
        let ev2 = tick(t - 3600, 3600, None, Some("07:00"), &mut st2);
        assert!(ev2.is_empty());
    }

    #[test]
    fn wake_invalide_ignore() {
        let mut st = AlarmState::new();
        assert!(tick(NOON, 0, None, Some("99:99"), &mut st).is_empty());
        assert!(tick(NOON, 0, None, None, &mut st).is_empty());
    }

    #[test]
    fn sleep_et_wake_peuvent_coincider() {
        let mut st = AlarmState::new();
        let t = (NOON / 86_400) * 86_400 + 12 * 3600; // 12:00 UTC
        let ev = tick(t, 0, Some(1), Some("12:00"), &mut st);
        // sleep armé pour +1 min (pas encore échu), wake à 12:00 -> Wake seul
        assert_eq!(ev, vec![Alarm::Wake]);
        let ev = tick(t + 60, 0, Some(1), Some("12:00"), &mut st);
        assert_eq!(ev, vec![Alarm::Sleep]);
    }
}
