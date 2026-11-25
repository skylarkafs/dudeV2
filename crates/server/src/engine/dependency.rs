use nextdude_common::proto::v1;
use std::collections::{HashMap, HashSet, VecDeque};

pub fn would_create_cycle(
    deps: &[v1::Dependency],
    new_parent: i64,
    new_child: i64,
) -> bool {
    if new_parent == new_child {
        return true;
    }
    let mut children_by_parent: HashMap<i64, Vec<i64>> = HashMap::new();
    for d in deps {
        children_by_parent
            .entry(d.parent_device_id)
            .or_default()
            .push(d.child_device_id);
    }
    children_by_parent
        .entry(new_parent)
        .or_default()
        .push(new_child);

    // BFS from new_child to see if new_parent is reachable.
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(new_child);
    while let Some(curr) = queue.pop_front() {
        if curr == new_parent {
            return true;
        }
        if visited.insert(curr) {
            if let Some(nexts) = children_by_parent.get(&curr) {
                for &n in nexts {
                    queue.push_back(n);
                }
            }
        }
    }
    false
}

#[derive(Debug, Default)]
pub struct SuppressionSummary {
    /// Child device IDs suppressed as Unreachable -> root parent device ID.
    pub suppressed_children: HashMap<i64, i64>,
    /// Root down parent device ID -> number of suppressed dependent devices.
    pub affected_counts_by_root: HashMap<i64, usize>,
}

/// Computes which devices are unreachable because an upstream dependency is down.
pub fn compute_dependency_suppression(
    deps: &[v1::Dependency],
    down_devices: &HashSet<i64>,
) -> SuppressionSummary {
    let mut children_by_parent: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut parents_by_child: HashMap<i64, Vec<i64>> = HashMap::new();
    for d in deps {
        children_by_parent
            .entry(d.parent_device_id)
            .or_default()
            .push(d.child_device_id);
        parents_by_child
            .entry(d.child_device_id)
            .or_default()
            .push(d.parent_device_id);
    }

    // Root causes are down devices that do NOT themselves have a down ancestor.
    let mut root_causes = Vec::new();
    for &dev_id in down_devices {
        let has_down_parent = parents_by_child
            .get(&dev_id)
            .map_or(false, |ps| ps.iter().any(|p| down_devices.contains(p)));
        if !has_down_parent {
            root_causes.push(dev_id);
        }
    }

    let mut summary = SuppressionSummary::default();
    for root in root_causes {
        let mut seen = HashSet::new();
        let mut q = VecDeque::new();
        if let Some(kids) = children_by_parent.get(&root) {
            for &k in kids {
                q.push_back(k);
            }
        }
        let mut count = 0usize;
        while let Some(child) = q.pop_front() {
            if !seen.insert(child) {
                continue;
            }
            summary.suppressed_children.entry(child).or_insert(root);
            count += 1;
            if let Some(next_kids) = children_by_parent.get(&child) {
                for &nk in next_kids {
                    q.push_back(nk);
                }
            }
        }
        if count > 0 {
            summary.affected_counts_by_root.insert(root, count);
        }
    }
    summary
}
