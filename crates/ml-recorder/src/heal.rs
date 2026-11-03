//! Output ids that Windows replaced. An update or a driver rebuild can give a device a new endpoint id without it
//! ever being unplugged (03.11.2026: KB5127936 re-created "Arctis 5 Game", its Chat half kept its id). Settings
//! keyed by the old id (shortcut cycle, hidden outputs, EQ) then point at nothing. Every output seen is remembered
//! by name in `outputs-seen.json`; a remembered id that is gone moves to the one NEW output with the same name.
//! Two candidates (two identical headsets) or none: nothing is guessed.

use crate::pick::Endpoint;
use crate::session::Shared;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Default, Serialize, Deserialize)]
struct Seen {
    /// endpoint id -> friendly name, every output seen while plugged in
    #[serde(default)]
    names: BTreeMap<String, String>,
    /// old id -> the id it moved to; also re-applied to stale copies the app writes back later
    #[serde(default)]
    moved: BTreeMap<String, String>,
}

fn path() -> std::path::PathBuf {
    crate::config::data_dir().join("outputs-seen.json")
}

/// Old id -> new id for every referenced id that is no longer plugged in.
fn remap(refs: &[String], seen: &mut Seen, present: &[Endpoint]) -> BTreeMap<String, String> {
    let is_present = |id: &str| present.iter().any(|e| e.id == id);
    let mut out = BTreeMap::new();
    for old in refs {
        if is_present(old) || out.contains_key(old) {
            continue;
        }
        if let Some(new) = seen.moved.get(old).filter(|n| is_present(n)) {
            out.insert(old.clone(), new.clone());
            continue;
        }
        let Some(name) = seen.names.get(old) else { continue };
        let fresh: Vec<&Endpoint> = present
            .iter()
            .filter(|e| &e.name == name && !seen.names.contains_key(&e.id) && !refs.contains(&e.id))
            .collect();
        if let [e] = fresh[..] {
            seen.moved.insert(old.clone(), e.id.clone());
            out.insert(old.clone(), e.id.clone());
        }
    }
    out
}

fn apply_list(list: &mut Vec<String>, map: &BTreeMap<String, String>) -> bool {
    let before = list.clone();
    let mut next: Vec<String> = Vec::new();
    for id in list.iter() {
        let id = map.get(id).unwrap_or(id).clone();
        if !next.contains(&id) {
            next.push(id);
        }
    }
    *list = next;
    *list != before
}

/// Rewrites only the two keys in recorder.json (the app owns the rest of the file).
fn patch_config(cycle: &[String], hidden: &[String]) {
    let p = crate::config::config_path();
    let Ok(bytes) = std::fs::read(&p) else { return };
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return };
    let Some(obj) = v.as_object_mut() else { return };
    obj.insert("output_cycle".into(), serde_json::json!(cycle));
    obj.insert("hidden_outputs".into(), serde_json::json!(hidden));
    if let Ok(b) = serde_json::to_vec_pretty(&v) {
        let _ = ml_eq::write_atomic(&p, &b);
    }
}

/// Run every few seconds on the tray thread.
pub fn check(shared: &Shared) {
    let present = crate::endpoints::outputs();
    if present.is_empty() {
        return;
    }
    let mut seen: Seen = std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let (mut cycle, mut hidden) = {
        let c = shared.cfg.lock().unwrap();
        (c.output_cycle.clone(), c.hidden_outputs.clone())
    };
    let eq_ids: Vec<String> = shared.eq.lock().unwrap().outputs.keys().cloned().collect();
    let refs: Vec<String> = cycle.iter().chain(&hidden).chain(&eq_ids).cloned().collect();
    let map = remap(&refs, &mut seen, &present);
    let mut dirty = false;
    for e in &present {
        if seen.names.get(&e.id) != Some(&e.name) {
            seen.names.insert(e.id.clone(), e.name.clone());
            dirty = true;
        }
    }
    if !map.is_empty() {
        dirty = true;
        crate::session::log(&format!("output ids moved: {map:?}"));
        if apply_list(&mut cycle, &map) | apply_list(&mut hidden, &map) {
            {
                let mut c = shared.cfg.lock().unwrap();
                c.output_cycle = cycle.clone();
                c.hidden_outputs = hidden.clone();
            }
            patch_config(&cycle, &hidden);
        }
        if eq_ids.iter().any(|id| map.contains_key(id)) {
            crate::eq::update(shared, |st| {
                for (old, new) in &map {
                    // a channel already under the new id (set up meanwhile) wins over the old one
                    if let Some(ch) = st.outputs.remove(old) {
                        st.outputs.entry(new.clone()).or_insert(ch);
                    }
                }
            });
        }
    }
    if dirty {
        if let Ok(b) = serde_json::to_vec_pretty(&seen) {
            let _ = ml_eq::write_atomic(&path(), &b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(id: &str, name: &str) -> Endpoint {
        Endpoint { id: id.into(), name: name.into() }
    }

    fn seen(names: &[(&str, &str)]) -> Seen {
        Seen { names: names.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(), moved: BTreeMap::new() }
    }

    #[test]
    fn a_gone_id_moves_to_the_one_new_output_with_its_name() {
        let mut s = seen(&[("old", "Game"), ("mon", "Monitor")]);
        let present = [ep("new", "Game"), ep("mon", "Monitor")];
        let m = remap(&["old".into(), "mon".into()], &mut s, &present);
        assert_eq!(m.get("old").map(String::as_str), Some("new"));
        assert_eq!(m.len(), 1);
        assert_eq!(s.moved.get("old").map(String::as_str), Some("new"));
    }

    #[test]
    fn two_candidates_or_a_known_one_are_not_guessed() {
        let mut s = seen(&[("old", "Game")]);
        let two = [ep("a", "Game"), ep("b", "Game")];
        assert!(remap(&["old".into()], &mut s, &two).is_empty());
        // an output seen before is another device that shares the name, not the replacement
        let mut s = seen(&[("old", "Game"), ("other", "Game")]);
        assert!(remap(&["old".into()], &mut s, &[ep("other", "Game")]).is_empty());
    }

    #[test]
    fn an_unplugged_device_with_no_replacement_stays() {
        let mut s = seen(&[("old", "Game")]);
        assert!(remap(&["old".into()], &mut s, &[ep("mon", "Monitor")]).is_empty());
    }

    #[test]
    fn a_remembered_move_fixes_a_stale_copy_again() {
        let mut s = seen(&[("old", "Game"), ("new", "Game")]);
        s.moved.insert("old".into(), "new".into());
        let m = remap(&["old".into()], &mut s, &[ep("new", "Game")]);
        assert_eq!(m.get("old").map(String::as_str), Some("new"));
    }

    #[test]
    fn lists_are_rewritten_without_duplicates() {
        let map: BTreeMap<String, String> = [("old".to_string(), "new".to_string())].into();
        let mut l = vec!["old".to_string(), "new".to_string(), "mon".to_string()];
        assert!(apply_list(&mut l, &map));
        assert_eq!(l, ["new", "mon"]);
    }
}
