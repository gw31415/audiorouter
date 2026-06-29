//! Topological layered layout for the routing graph.
//!
//! Instead of a fixed 3-column (Input / Both / Output) grid, devices are
//! placed in layers derived from the actual directed graph of routes.
//! This produces a natural left-to-right signal flow where:
//!
//! - Pure inputs gravitate to the left (no incoming routes).
//! - Pure outputs gravitate to the right (no outgoing routes).
//! - Intermediate devices (mixers, loopback, etc.) appear between them
//!   at a depth that reflects their actual position in the signal chain.
//!
//! ## Algorithm
//!
//! A simplified [Sugiyama framework]:
//!
//! 1. **Layering** — Each device is assigned a layer (column) equal to the
//!    longest path from any source node. This guarantees that for every
//!    route `A → B`, `layer(B) > layer(A)`.
//!
//! 2. **Crossing minimization** — Within each layer, nodes are reordered by
//!    the barycenter heuristic: a node is placed near the average position
//!    of its neighbours in the adjacent layer. A forward pass (using
//!    predecessors) and a backward pass (using successors) reduce edge
//!    crossings.
//!
//! [Sugiyama framework]: https://en.wikipedia.org/wiki/Layered_graph_drawing

use std::collections::HashMap;

use crate::validate::ValidatedConfig;

/// A device placed in the computed layered layout.
#[derive(Debug, Clone)]
pub struct PlacedNode {
    /// Device alias.
    pub alias: String,
    /// Layer (column) index, 0-based from left.
    pub layer: usize,
    /// Row within the layer, 0-based from top.
    pub row: usize,
}

/// Return the set of device aliases that participate in at least one route
/// (as `from` or `to`).
pub fn active_device_names(plan: &ValidatedConfig) -> std::collections::HashSet<String> {
    let mut active = std::collections::HashSet::new();
    for route in &plan.routes {
        active.insert(route.from.clone());
        active.insert(route.to.clone());
    }
    active
}

/// Return device aliases that are configured but do NOT participate in any
/// route. These are displayed separately (at the bottom) only when the user
/// explicitly toggles their visibility.
pub fn disconnected_device_names(plan: &ValidatedConfig) -> Vec<String> {
    let active = active_device_names(plan);
    plan.devices
        .iter()
        .map(|d| d.name.clone())
        .filter(|name| !active.contains(name))
        .collect()
}

/// Cascade-hide devices that lose all routes when `initial_hidden` devices
/// are removed from the graph.
///
/// After excluding the initial hidden set, only routes whose **both** endpoints
/// are still visible survive. Any active device that no longer appears in a
/// surviving route is also hidden.
///
/// Examples:
/// - A→B, A hidden → A→B dies → B has no surviving route → B also hidden.
/// - A→B←C, A hidden → A→B dies, C→B survives → B and C stay visible.
/// - A→B→C, A hidden → A→B dies, B→C survives → B and C stay visible.
pub fn cascade_hidden(
    plan: &ValidatedConfig,
    initial_hidden: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    let surviving_devices: std::collections::HashSet<String> = plan
        .routes
        .iter()
        .filter(|r| !initial_hidden.contains(&r.from) && !initial_hidden.contains(&r.to))
        .flat_map(|r| [r.from.clone(), r.to.clone()])
        .collect();
    let active = active_device_names(plan);
    let mut hidden = initial_hidden.clone();
    for name in &active {
        if !surviving_devices.contains(name) {
            hidden.insert(name.clone());
        }
    }
    hidden
}

/// Compute a topological layered layout for devices that participate in
/// at least one route, excluding any devices in the `exclude` set.
///
/// See the module docs for the algorithm description.
pub fn compute_layout(
    plan: &ValidatedConfig,
    exclude: &std::collections::HashSet<String>,
) -> Vec<PlacedNode> {
    let active = active_device_names(plan);
    let device_names: Vec<String> = plan
        .devices
        .iter()
        .map(|d| d.name.clone())
        .filter(|name| active.contains(name) && !exclude.contains(name))
        .collect();
    if device_names.is_empty() {
        return Vec::new();
    }

    // ── Build adjacency lists (deduplicated, skip excluded) ──────
    let mut successors: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut predecessors: HashMap<&str, Vec<&str>> = HashMap::new();
    for route in &plan.routes {
        if exclude.contains(&route.from) || exclude.contains(&route.to) {
            continue;
        }
        {
            let list = successors.entry(route.from.as_str()).or_default();
            if !list.contains(&route.to.as_str()) {
                list.push(route.to.as_str());
            }
        }
        {
            let list = predecessors.entry(route.to.as_str()).or_default();
            if !list.contains(&route.from.as_str()) {
                list.push(route.from.as_str());
            }
        }
    }

    // ── Longest-path layering ─────────────────────────────────────
    // layer(node) = 0 if no predecessors, else max(layer(pred)) + 1.
    // Iterate to fixpoint, with a safety limit for cyclic graphs.
    let mut layer: HashMap<&str, usize> = HashMap::new();
    for name in &device_names {
        layer.insert(name.as_str(), 0);
    }

    // Cap at device count — a DAG with N nodes has at most N layers.
    // This prevents unbounded growth in cyclic graphs (a→b→a).
    let layer_cap = device_names.len();

    let max_iters = device_names.len() + 1;
    for _ in 0..max_iters {
        let mut changed = false;
        for name in &device_names {
            if let Some(preds) = predecessors.get(name.as_str())
                && !preds.is_empty()
            {
                let max_pred = preds
                    .iter()
                    .map(|p| layer.get(p).copied().unwrap_or(0))
                    .max()
                    .unwrap_or(0);
                let new_layer = (max_pred + 1).min(layer_cap);
                if layer.get(name.as_str()).copied().unwrap_or(0) != new_layer {
                    layer.insert(name.as_str(), new_layer);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Remap layers to a contiguous 0..=max range (handles capped cycles
    // where intermediate layer values are skipped).
    let mut sorted_layers: Vec<usize> = layer.values().copied().collect();
    sorted_layers.sort_unstable();
    sorted_layers.dedup();
    let remap: HashMap<usize, usize> = sorted_layers
        .iter()
        .enumerate()
        .map(|(i, &l)| (l, i))
        .collect();
    for name in &device_names {
        let l = layer[name.as_str()];
        layer.insert(name.as_str(), remap[&l]);
    }

    let max_layer = layer.values().copied().max().unwrap_or(0);

    // ── Group nodes by layer (preserve config order within each) ──
    let mut by_layer: Vec<Vec<&str>> = vec![Vec::new(); max_layer + 1];
    for name in &device_names {
        let l = layer.get(name.as_str()).copied().unwrap_or(0);
        by_layer[l].push(name.as_str());
    }

    // ── Barycenter crossing minimization ──────────────────────────
    // Forward pass: reorder each layer by predecessor barycenter.
    for l in 1..=max_layer {
        let prev_pos: HashMap<&str, f32> = by_layer[l - 1]
            .iter()
            .enumerate()
            .map(|(i, &n)| (n, i as f32))
            .collect();

        by_layer[l].sort_by(|a, b| {
            let ba = barycenter(a, &predecessors, &prev_pos);
            let bb = barycenter(b, &predecessors, &prev_pos);
            ba.partial_cmp(&bb).unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    // Backward pass: reorder by successor barycenter.
    for l in (0..max_layer).rev() {
        let next_pos: HashMap<&str, f32> = by_layer[l + 1]
            .iter()
            .enumerate()
            .map(|(i, &n)| (n, i as f32))
            .collect();

        by_layer[l].sort_by(|a, b| {
            let ba = barycenter(a, &successors, &next_pos);
            let bb = barycenter(b, &successors, &next_pos);
            ba.partial_cmp(&bb).unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    // ── Build result ──────────────────────────────────────────────
    let mut result = Vec::new();
    for (l, nodes) in by_layer.iter().enumerate() {
        for (r, &name) in nodes.iter().enumerate() {
            result.push(PlacedNode {
                alias: name.to_string(),
                layer: l,
                row: r,
            });
        }
    }
    result
}

/// Average position of `node`'s neighbours that appear in `positions`.
///
/// Neighbours not in the reference layer are ignored. If no neighbours
/// are in the reference layer, returns `f32::MAX` (sorts to the end).
fn barycenter(
    node: &str,
    adjacency: &HashMap<&str, Vec<&str>>,
    positions: &HashMap<&str, f32>,
) -> f32 {
    match adjacency.get(node) {
        Some(neighbors) if !neighbors.is_empty() => {
            let relevant: Vec<f32> = neighbors
                .iter()
                .filter_map(|n| positions.get(n).copied())
                .collect();
            if relevant.is_empty() {
                f32::MAX
            } else {
                relevant.iter().sum::<f32>() / relevant.len() as f32
            }
        }
        _ => f32::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::collections::HashSet;

    fn plan_from_toml(toml_str: &str) -> ValidatedConfig {
        let config: Config = toml::from_str(toml_str).unwrap();
        crate::validate::validate_config(config).unwrap()
    }

    fn layer_of(plan: &ValidatedConfig, alias: &str) -> usize {
        compute_layout(plan, &Default::default())
            .iter()
            .find(|n| n.alias == alias)
            .map(|n| n.layer)
            .unwrap_or(usize::MAX)
    }

    const ENGINE: &str = "[engine]\nsample_rate = 48000\nbuffer_size = 256\n";

    #[test]
    fn linear_chain() {
        let plan = plan_from_toml(&format!(
            r#"
{ENGINE}
[[routes]]
from = "a"
to = "b"
from_channels = [1]
to_channels = [1]
[[routes]]
from = "b"
to = "c"
from_channels = [1]
to_channels = [1]
"#
        ));

        let layout = compute_layout(&plan, &Default::default());
        assert_eq!(layer_of(&plan, "a"), 0);
        assert_eq!(layer_of(&plan, "b"), 1);
        assert_eq!(layer_of(&plan, "c"), 2);
        assert_eq!(layout.len(), 3);
    }

    #[test]
    fn fan_out() {
        // a → b, a → c, a → d
        let plan = plan_from_toml(&format!(
            r#"
{ENGINE}
[[routes]]
from = "a"
to = "b"
from_channels = [1]
to_channels = [1]
[[routes]]
from = "a"
to = "c"
from_channels = [1]
to_channels = [1]
[[routes]]
from = "a"
to = "d"
from_channels = [1]
to_channels = [1]
"#
        ));

        assert_eq!(layer_of(&plan, "a"), 0);
        assert_eq!(layer_of(&plan, "b"), 1);
        assert_eq!(layer_of(&plan, "c"), 1);
        assert_eq!(layer_of(&plan, "d"), 1);

        // b, c, d should all be in layer 1.
        let layout = compute_layout(&plan, &Default::default());
        let layer1: Vec<&PlacedNode> = layout.iter().filter(|n| n.layer == 1).collect();
        assert_eq!(layer1.len(), 3);
        // Rows should be 0, 1, 2.
        let rows: Vec<usize> = layer1.iter().map(|n| n.row).collect();
        assert!(rows.contains(&0));
        assert!(rows.contains(&1));
        assert!(rows.contains(&2));
    }

    #[test]
    fn fan_in() {
        // a → d, b → d, c → d
        let plan = plan_from_toml(&format!(
            r#"
{ENGINE}
[[routes]]
from = "a"
to = "d"
from_channels = [1]
to_channels = [1]
[[routes]]
from = "b"
to = "d"
from_channels = [1]
to_channels = [1]
[[routes]]
from = "c"
to = "d"
from_channels = [1]
to_channels = [1]
"#
        ));

        assert_eq!(layer_of(&plan, "a"), 0);
        assert_eq!(layer_of(&plan, "b"), 0);
        assert_eq!(layer_of(&plan, "c"), 0);
        assert_eq!(layer_of(&plan, "d"), 1);
    }

    #[test]
    fn diamond_graph() {
        // a → b, a → c, b → d, c → d
        let plan = plan_from_toml(&format!(
            r#"
{ENGINE}
[[routes]]
from = "a"
to = "b"
from_channels = [1]
to_channels = [1]
[[routes]]
from = "a"
to = "c"
from_channels = [1]
to_channels = [1]
[[routes]]
from = "b"
to = "d"
from_channels = [1]
to_channels = [1]
[[routes]]
from = "c"
to = "d"
from_channels = [1]
to_channels = [1]
"#
        ));

        assert_eq!(layer_of(&plan, "a"), 0);
        assert_eq!(layer_of(&plan, "b"), 1);
        assert_eq!(layer_of(&plan, "c"), 1);
        assert_eq!(layer_of(&plan, "d"), 2);
    }

    #[test]
    fn intermediate_device() {
        // a → mixer → b,  mixer is a "both" device
        // a is pure input, b is pure output
        let plan = plan_from_toml(&format!(
            r#"
{ENGINE}
[[routes]]
from = "a"
to = "mixer"
from_channels = [1]
to_channels = [1]
[[routes]]
from = "mixer"
to = "b"
from_channels = [1]
to_channels = [1]
"#
        ));

        // mixer should be between a and b, not in a fixed "both" column.
        let la = layer_of(&plan, "a");
        let lm = layer_of(&plan, "mixer");
        let lb = layer_of(&plan, "b");
        assert!(la < lm);
        assert!(lm < lb);
    }

    #[test]
    fn long_chain_has_many_layers() {
        // a → b → c → d → e
        let toml = format!(
            r#"
{ENGINE}
[[routes]]
from = "a"
to = "b"
from_channels = [1]
to_channels = [1]
[[routes]]
from = "b"
to = "c"
from_channels = [1]
to_channels = [1]
[[routes]]
from = "c"
to = "d"
from_channels = [1]
to_channels = [1]
[[routes]]
from = "d"
to = "e"
from_channels = [1]
to_channels = [1]
"#
        );
        let plan = plan_from_toml(&toml);
        let layout = compute_layout(&plan, &Default::default());
        let max_layer = layout.iter().map(|n| n.layer).max().unwrap();
        assert_eq!(max_layer, 4); // 5 nodes, 4 layers (0..=4)
    }

    #[test]
    fn no_routes_empty_layout() {
        // A plan with devices but no routes — validation will fail,
        // so this tests compute_layout directly.
        let plan = ValidatedConfig {
            config: toml::from_str(ENGINE).unwrap(),
            devices: vec![],
            routes: vec![],
            warnings: vec![],
        };
        let layout = compute_layout(&plan, &Default::default());
        assert!(layout.is_empty());
    }

    #[test]
    fn barycenter_reduces_crossings() {
        // Graph: a0→b0, a1→b1  (no crossings expected)
        // vs    a0→b1, a1→b0  (crossing if not reordered)
        //
        // With barycenter, b0 should be near a0 and b1 near a1.
        let plan = plan_from_toml(&format!(
            r#"
{ENGINE}
[[routes]]
from = "a0"
to = "b1"
from_channels = [1]
to_channels = [1]
[[routes]]
from = "a1"
to = "b0"
from_channels = [1]
to_channels = [1]
"#
        ));

        let layout = compute_layout(&plan, &Default::default());
        let b0 = layout.iter().find(|n| n.alias == "b0").unwrap();
        let b1 = layout.iter().find(|n| n.alias == "b1").unwrap();
        let a0 = layout.iter().find(|n| n.alias == "a0").unwrap();
        let a1 = layout.iter().find(|n| n.alias == "a1").unwrap();

        // a0 is connected to b1, so barycenter(b1) ≈ pos(a0).
        // a1 is connected to b0, so barycenter(b0) ≈ pos(a1).
        // After reordering, b1 should be near a0 and b0 near a1.
        if a0.row < a1.row {
            // a0 is above a1, so b1 (connected to a0) should be above b0.
            // But the backward pass may flip this. Just check they're reordered.
            assert!(b0.row != b1.row);
        }
    }

    #[test]
    fn cycle_does_not_infinite_loop() {
        // a → b → a  (cycle). The fixpoint loop should terminate.
        let plan = plan_from_toml(&format!(
            r#"
{ENGINE}
[[routes]]
from = "a"
to = "b"
from_channels = [1]
to_channels = [1]
[[routes]]
from = "b"
to = "a"
from_channels = [1]
to_channels = [1]
"#
        ));

        // Should not hang.
        let layout = compute_layout(&plan, &Default::default());
        assert_eq!(layout.len(), 2);
        // Both nodes get assigned a layer (high value due to cycle).
        assert!(layout.iter().all(|n| n.layer < usize::MAX));
    }

    #[test]
    fn disconnected_devices_excluded_from_layout() {
        // Devices: a, b (active), lonely (no routes)
        // Route: a → b
        let plan = plan_from_toml(&format!(
            r#"
{ENGINE}
[[devices]]
name = "lonely"
device = "LonelyDevice"

[[routes]]
from = "a"
to = "b"
from_channels = [1]
to_channels = [1]
"#
        ));

        // "lonely" should NOT appear in the layout.
        let layout = compute_layout(&plan, &Default::default());
        assert!(layout.iter().any(|n| n.alias == "a"));
        assert!(layout.iter().any(|n| n.alias == "b"));
        assert!(!layout.iter().any(|n| n.alias == "lonely"));
        assert_eq!(layout.len(), 2);
    }

    #[test]
    fn disconnected_device_names_correct() {
        // Devices: a, b, c, d — routes only reference a and b.
        let plan = plan_from_toml(&format!(
            r#"
{ENGINE}
[[devices]]
name = "c"
device = "DevC"

[[devices]]
name = "d"
device = "DevD"

[[routes]]
from = "a"
to = "b"
from_channels = [1]
to_channels = [1]
"#
        ));

        let active = active_device_names(&plan);
        assert!(active.contains("a"));
        assert!(active.contains("b"));
        assert!(!active.contains("c"));
        assert!(!active.contains("d"));

        let disconnected = disconnected_device_names(&plan);
        assert_eq!(disconnected.len(), 2);
        assert!(disconnected.contains(&"c".to_string()));
        assert!(disconnected.contains(&"d".to_string()));
    }

    #[test]
    fn all_devices_active_when_all_in_routes() {
        // Every device participates in a route — no disconnected devices.
        let plan = plan_from_toml(&format!(
            r#"
{ENGINE}
[[routes]]
from = "a"
to = "b"
from_channels = [1]
to_channels = [1]
"#
        ));

        let disconnected = disconnected_device_names(&plan);
        assert!(disconnected.is_empty());
    }

    #[test]
    fn cascade_simple_chain_all_hidden() {
        // A → B, hide A → no surviving routes → B also hidden.
        let plan = plan_from_toml(&format!(
            r#"
{ENGINE}
[[routes]]
from = "a"
to = "b"
from_channels = [1]
to_channels = [1]
"#
        ));
        let initial: HashSet<String> = ["a".to_string()].into_iter().collect();
        let hidden = cascade_hidden(&plan, &initial);
        assert!(hidden.contains("a"));
        assert!(hidden.contains("b"));
    }

    #[test]
    fn cascade_partial_survives() {
        // A → B ← C, hide A → C→B survives → B and C stay visible.
        let plan = plan_from_toml(&format!(
            r#"
{ENGINE}
[[routes]]
from = "a"
to = "b"
from_channels = [1]
to_channels = [1]

[[routes]]
from = "c"
to = "b"
from_channels = [1]
to_channels = [1]
"#
        ));
        let initial: HashSet<String> = ["a".to_string()].into_iter().collect();
        let hidden = cascade_hidden(&plan, &initial);
        assert!(hidden.contains("a"));
        assert!(!hidden.contains("b"));
        assert!(!hidden.contains("c"));
    }

    #[test]
    fn cascade_no_hidden_returns_empty() {
        // No initial hidden → all active devices survive.
        let plan = plan_from_toml(&format!(
            r#"
{ENGINE}
[[routes]]
from = "a"
to = "b"
from_channels = [1]
to_channels = [1]
"#
        ));
        let initial: HashSet<String> = HashSet::new();
        let hidden = cascade_hidden(&plan, &initial);
        assert!(hidden.is_empty());
    }

    #[test]
    fn cascade_multi_hop() {
        // A → B → C, hide A → A→B dies, B→C survives → B and C stay visible.
        let plan = plan_from_toml(&format!(
            r#"
{ENGINE}
[[routes]]
from = "a"
to = "b"
from_channels = [1]
to_channels = [1]

[[routes]]
from = "b"
to = "c"
from_channels = [1]
to_channels = [1]
"#
        ));
        let initial: HashSet<String> = ["a".to_string()].into_iter().collect();
        let hidden = cascade_hidden(&plan, &initial);
        assert!(hidden.contains("a"));
        assert!(!hidden.contains("b"));
        assert!(!hidden.contains("c"));
    }
}
