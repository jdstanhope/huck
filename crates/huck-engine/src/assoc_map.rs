use std::collections::HashMap;

/// An order-preserving string→string map for associative arrays. `order`
/// holds `(key, value)` in INSERTION order (the basis the L-44 view sorts on);
/// `index` maps each live key to its position in `order` for O(1) access.
/// Invariant: `index[k] == i` iff `order[i].0 == k`, and `order` has no
/// duplicate keys. Insertion order of survivors is preserved across updates
/// and removes (matching bash), so `assoc_order.rs` output is unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssocMap {
    order: Vec<(String, String)>,
    index: HashMap<String, usize>,
}

impl AssocMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// O(1) get.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.index.get(key).map(|&i| self.order[i].1.as_str())
    }
    pub fn contains_key(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }
    pub fn len(&self) -> usize {
        self.order.len()
    }
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// O(1): update in place if present (position unchanged, like bash), else
    /// append (new key = highest insertion index).
    pub fn insert(&mut self, key: String, value: String) {
        if let Some(&i) = self.index.get(&key) {
            self.order[i].1 = value;
        } else {
            self.index.insert(key.clone(), self.order.len());
            self.order.push((key, value));
        }
    }

    /// O(n): shift-remove, reindexing the tail so survivors keep their relative
    /// insertion order (matching bash `unset a[k]`). Returns whether present.
    pub fn remove(&mut self, key: &str) -> bool {
        match self.index.remove(key) {
            Some(i) => {
                self.order.remove(i);
                for (j, (k, _)) in self.order.iter().enumerate().skip(i) {
                    self.index.insert(k.clone(), j);
                }
                true
            }
            None => false,
        }
    }

    /// The `(key, value)` pairs in INSERTION order (what the L-44 view consumes).
    pub fn pairs(&self) -> &[(String, String)] {
        &self.order
    }
    pub fn iter(&self) -> std::slice::Iter<'_, (String, String)> {
        self.order.iter()
    }
}

impl FromIterator<(String, String)> for AssocMap {
    /// Build from `(k,v)` pairs in order; a later duplicate key updates in place
    /// (keeping the FIRST position), matching bash compound-assignment semantics.
    fn from_iter<I: IntoIterator<Item = (String, String)>>(it: I) -> Self {
        let mut m = AssocMap::new();
        for (k, v) in it {
            m.insert(k, v);
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_update_len() {
        let mut m = AssocMap::new();
        m.insert("a".into(), "1".into());
        m.insert("b".into(), "2".into());
        assert_eq!(m.get("a"), Some("1"));
        assert_eq!(m.len(), 2);
        m.insert("a".into(), "9".into()); // update in place
        assert_eq!(m.get("a"), Some("9"));
        assert_eq!(m.len(), 2);
        assert_eq!(
            m.pairs(),
            &[("a".into(), "9".into()), ("b".into(), "2".into())]
        ); // a keeps pos
    }

    #[test]
    fn remove_preserves_order() {
        let mut m: AssocMap = [("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert!(m.remove("b"));
        assert!(!m.remove("zzz"));
        assert_eq!(
            m.pairs()
                .iter()
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "c", "d"]
        );
        // index still correct after reindex
        assert_eq!(m.get("d"), Some("4"));
        m.insert("e".into(), "5".into());
        assert_eq!(m.pairs().last().unwrap().0, "e");
    }

    #[test]
    fn from_iter_dup_key_first_pos_last_value() {
        // bash: declare -A a=([k]=1 [x]=2 [k]=3) -> k keeps first pos, value 3.
        let m: AssocMap = [("k", "1"), ("x", "2"), ("k", "3")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(
            m.pairs()
                .iter()
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>(),
            vec!["k", "x"]
        );
        assert_eq!(m.get("k"), Some("3"));
    }

    #[test]
    fn property_matches_vec_reference() {
        // Random op sequences: AssocMap must match a Vec-based reference for
        // both key-set and ordered pairs. Deterministic PRNG (no external dep).
        fn vec_ref_insert(v: &mut Vec<(String, String)>, k: String, val: String) {
            if let Some(s) = v.iter_mut().find(|(kk, _)| *kk == k) {
                s.1 = val;
            } else {
                v.push((k, val));
            }
        }
        fn vec_ref_remove(v: &mut Vec<(String, String)>, k: &str) {
            v.retain(|(kk, _)| kk != k);
        }
        let mut seed: u64 = 0x9e3779b97f4a7c15;
        let mut rng = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..300 {
            let mut m = AssocMap::new();
            let mut r: Vec<(String, String)> = Vec::new();
            for _ in 0..200 {
                let key = format!("k{}", rng() % 30);
                if rng() % 4 == 0 {
                    m.remove(&key);
                    vec_ref_remove(&mut r, &key);
                } else {
                    let val = (rng() % 1000).to_string();
                    m.insert(key.clone(), val.clone());
                    vec_ref_insert(&mut r, key, val);
                }
                assert_eq!(
                    m.pairs(),
                    r.as_slice(),
                    "ordered pairs must match Vec reference"
                );
            }
        }
    }
}
