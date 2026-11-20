//! Projection for the export in paperless-ngx v3.4.2 format: compute the derived django-treenode
//! fields (`tn_ancestors_pks` … `tn_siblings_count`) from the adjacency list. PaperRust
//! no longer stores them since `pl_objects.0001_paperrust_tag_adjacency` (tree logic:
//! [`crate::tagtree`]); paperless-ngx 3.4.2 still needs them when importing an export.
//!
//! upstream: django-treenode 0.24.0 treenode/models.py (TreeNodeModel.update_tree,
//!           __get_nodes_data, __get_node_data, __get_node_order_str)

use std::collections::HashMap;

use crate::text::slugify;

/// The derived django-treenode columns of a tag (v3.4.2 schema).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeFields {
    pub tn_ancestors_pks: String,
    pub tn_ancestors_count: i64,
    pub tn_children_pks: String,
    pub tn_children_count: i64,
    pub tn_depth: i64,
    pub tn_descendants_pks: String,
    pub tn_descendants_count: i64,
    pub tn_index: i64,
    pub tn_level: i64,
    pub tn_order: i64,
    pub tn_siblings_pks: String,
    pub tn_siblings_count: i64,
}
const PRIORITY_MAX: i64 = 9_999_999_999;
const PRIORITY_LEN: usize = 10;

/// Input node for the recomputation.
#[derive(Clone, Debug)]
pub struct Node {
    pub pk: i64,
    pub parent: Option<i64>,
    pub priority: i64,
    pub name: String,
}

/// `__get_node_order_str` (for one node).
fn order_str(n: &Node) -> String {
    let priority_val = PRIORITY_MAX - n.priority.min(PRIORITY_MAX);
    let priority_key = format!("{:0>width$}", priority_val, width = PRIORITY_LEN);
    let mut alpha: String = slugify(&n.name);
    while alpha.chars().count() < PRIORITY_LEN {
        alpha.push('z');
    }
    let alpha: String = alpha.chars().take(PRIORITY_LEN).collect();
    let pk_val = n.pk.min(PRIORITY_MAX);
    let pk_key = format!("{:0>width$}", pk_val, width = PRIORITY_LEN);
    format!("{priority_key}{alpha}{pk_key}").to_uppercase()
}

fn join_pks(v: &[i64]) -> String {
    v.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",")
}

struct Data {
    pk: i64,
    parent: Option<i64>,
    ancestors: Vec<i64>,
    children: Vec<i64>,
    descendants: Vec<i64>,
    siblings: Vec<i64>,
    depth: i64,
    level: i64,
    order: i64,
    index: i64,
    order_str: String,
}

/// `__get_nodes_data`: the full tree state for every node.
pub fn compute(nodes: &[Node]) -> HashMap<i64, TreeFields> {
    let by_pk: HashMap<i64, &Node> = nodes.iter().map(|n| (n.pk, n)).collect();
    let mut data: Vec<Data> = nodes
        .iter()
        .map(|n| {
            let mut ancestors: Vec<&Node> = Vec::new();
            let mut cur = n.parent;
            // Guard against cycles (upstream would loop forever; validation prevents them).
            let mut steps = 0;
            while let Some(p) = cur {
                let Some(a) = by_pk.get(&p) else { break };
                ancestors.insert(0, a);
                cur = a.parent;
                steps += 1;
                if steps > nodes.len() {
                    break;
                }
            }
            let mut os = String::new();
            for a in &ancestors {
                os.push_str(&order_str(a));
            }
            os.push_str(&order_str(n));
            Data {
                pk: n.pk,
                parent: n.parent,
                level: ancestors.len() as i64 + 1,
                ancestors: ancestors.iter().map(|a| a.pk).collect(),
                children: vec![],
                descendants: vec![],
                siblings: vec![],
                depth: 0,
                order: 0,
                index: 0,
                order_str: os,
            }
        })
        .collect();

    data.sort_by(|a, b| a.order_str.cmp(&b.order_str));

    let mut by_parent: HashMap<Option<i64>, Vec<i64>> = HashMap::new();
    let mut index_cursor: HashMap<Option<i64>, i64> = HashMap::new();
    for (order, d) in data.iter_mut().enumerate() {
        by_parent.entry(d.parent).or_default().push(d.pk);
        d.order = order as i64;
        let c = index_cursor.entry(d.parent).or_insert(0);
        d.index = *c;
        *c += 1;
    }

    let pos: HashMap<i64, usize> = data.iter().enumerate().map(|(i, d)| (d.pk, i)).collect();
    let mut by_level: Vec<usize> = (0..data.len()).collect();
    // sorted(..., key=level, reverse=True) is stable w.r.t. the order-sorted list
    by_level.sort_by(|a, b| data[*b].level.cmp(&data[*a].level));

    for i in by_level {
        let pk = data[i].pk;
        let parent = data[i].parent;
        let children = by_parent.get(&Some(pk)).cloned().unwrap_or_default();
        let mut siblings = by_parent.get(&parent).cloned().unwrap_or_default();
        if let Some(p) = siblings.iter().position(|s| *s == pk) {
            siblings.remove(p);
        }
        if !children.is_empty() {
            let mut desc = children.clone();
            let mut depth = 1;
            for c in &children {
                if let Some(ci) = pos.get(c) {
                    let cd = &data[*ci];
                    if !cd.descendants.is_empty() {
                        desc.extend(cd.descendants.iter().copied());
                        depth = depth.max(cd.depth + 1);
                    }
                }
            }
            desc.sort_by_key(|p| pos.get(p).map(|i| data[*i].order).unwrap_or(0));
            data[i].descendants = desc;
            data[i].depth = depth;
        }
        data[i].children = children;
        data[i].siblings = siblings;
    }

    data.into_iter()
        .map(|d| {
            (
                d.pk,
                TreeFields {
                    tn_ancestors_pks: join_pks(&d.ancestors),
                    tn_ancestors_count: d.ancestors.len() as i64,
                    tn_children_count: d.children.len() as i64,
                    tn_children_pks: join_pks(&d.children),
                    tn_depth: d.depth,
                    tn_descendants_count: d.descendants.len() as i64,
                    tn_descendants_pks: join_pks(&d.descendants),
                    tn_index: d.index,
                    tn_level: d.level,
                    tn_order: d.order,
                    tn_siblings_count: d.siblings.len() as i64,
                    tn_siblings_pks: join_pks(&d.siblings),
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(pk: i64, parent: Option<i64>, name: &str) -> Node {
        Node { pk, parent, priority: 0, name: name.into() }
    }

    #[test]
    fn order_string_format() {
        assert_eq!(order_str(&n(7, None, "Ab")), "9999999999ABZZZZZZZZ0000000007");
        assert_eq!(
            order_str(&Node { pk: 3, parent: None, priority: 5, name: "a very long tag name".into() }),
            "9999999994A-VERY-LON0000000003"
        );
    }

    #[test]
    fn simple_tree() {
        // Parent(1) <- Child(2) <- Grandchild(4); Other(3)
        let nodes = vec![n(1, None, "Parent"), n(2, Some(1), "Child"), n(3, None, "Other"), n(4, Some(2), "G")];
        let t = compute(&nodes);
        // order: Other < Parent (alphabetical) ; Parent's subtree follows Parent
        assert_eq!(t[&3].tn_order, 0);
        assert_eq!(t[&1].tn_order, 1);
        assert_eq!(t[&2].tn_order, 2);
        assert_eq!(t[&4].tn_order, 3);
        assert_eq!(t[&1].tn_children_pks, "2");
        assert_eq!(t[&1].tn_descendants_pks, "2,4");
        assert_eq!(t[&1].tn_depth, 2);
        assert_eq!(t[&2].tn_depth, 1);
        assert_eq!(t[&4].tn_ancestors_pks, "1,2");
        assert_eq!(t[&4].tn_level, 3);
        assert_eq!(t[&1].tn_siblings_pks, "3");
        assert_eq!(t[&3].tn_index, 0);
        assert_eq!(t[&1].tn_index, 1);
        assert_eq!(t[&2].tn_index, 0);
    }

    #[test]
    fn descendants_quirk_only_counts_grandchildren_via_nonempty() {
        // A(1) <- B(2), A <- C(3) ; C <- D(4)
        let nodes = vec![n(1, None, "A"), n(2, Some(1), "B"), n(3, Some(1), "C"), n(4, Some(3), "D")];
        let t = compute(&nodes);
        assert_eq!(t[&1].tn_descendants_pks, "2,3,4");
        assert_eq!(t[&1].tn_depth, 2);
        assert_eq!(t[&1].tn_descendants_count, 3);
    }
}

#[cfg(test)]
mod reference_tree {
    use super::*;

    /// Tree of the reference seed (documents_tag in reference/, written by django-treenode).
    #[test]
    #[allow(clippy::type_complexity)]
    fn matches_django_treenode_on_reference_seed() {
        let rows: &[(i64, Option<i64>, &str)] = &[
            (1, None, "Inbox"), (2, None, "Finance"), (3, Some(2), "Taxes"), (4, Some(3), "Taxes 2024"),
            (5, Some(3), "Taxes 2025"), (6, Some(2), "Invoices"), (7, Some(6), "Paid"), (8, Some(6), "Unpaid"),
            (9, Some(8), "Overdue"), (10, None, "Home"), (11, Some(10), "Rent"), (12, Some(10), "Utilities"),
            (13, None, "Health"), (14, None, "Insurance"), (15, None, "Vehicle"), (16, None, "Important"),
            (17, None, "To Do"), (18, None, "Archive"), (19, None, "Private"), (20, None, "Ümlaut-Tag ä"),
        ];
        let nodes: Vec<Node> = rows.iter().map(|(pk, p, n)| Node { pk: *pk, parent: *p, priority: 0, name: (*n).into() }).collect();
        let t = compute(&nodes);
        // (id, ancestors, children, descendants, order, index, level, depth, siblings) from the DB
        let expected: &[(i64, &str, &str, &str, i64, i64, i64, i64, &str)] = &[
            (18, "", "", "", 0, 0, 1, 0, "2,13,10,16,1,14,19,17,20,15"),
            (2, "", "6,3", "6,7,8,9,3,4,5", 1, 1, 1, 3, "18,13,10,16,1,14,19,17,20,15"),
            (6, "2", "7,8", "7,8,9", 2, 0, 2, 2, "3"),
            (7, "2,6", "", "", 3, 0, 3, 0, "8"),
            (8, "2,6", "9", "9", 4, 1, 3, 1, "7"),
            (9, "2,6,8", "", "", 5, 0, 4, 0, ""),
            (3, "2", "4,5", "4,5", 6, 1, 2, 1, "6"),
            (4, "2,3", "", "", 7, 0, 3, 0, "5"),
            (5, "2,3", "", "", 8, 1, 3, 0, "4"),
            (13, "", "", "", 9, 2, 1, 0, "18,2,10,16,1,14,19,17,20,15"),
            (10, "", "11,12", "11,12", 10, 3, 1, 1, "18,2,13,16,1,14,19,17,20,15"),
            (20, "", "", "", 18, 9, 1, 0, "18,2,13,10,16,1,14,19,17,15"),
            (15, "", "", "", 19, 10, 1, 0, "18,2,13,10,16,1,14,19,17,20"),
        ];
        for (id, anc, ch, de, order, index, level, depth, sib) in expected {
            let f = &t[id];
            assert_eq!(
                (f.tn_ancestors_pks.as_str(), f.tn_children_pks.as_str(), f.tn_descendants_pks.as_str(), f.tn_order, f.tn_index, f.tn_level, f.tn_depth, f.tn_siblings_pks.as_str()),
                (*anc, *ch, *de, *order, *index, *level, *depth, *sib),
                "tag {id}"
            );
        }
    }
}
