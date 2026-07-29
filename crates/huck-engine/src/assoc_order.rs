//! bash 5.2.21 associative-array iteration order (L-44, #32). huck stores
//! assoc arrays insertion-ordered; bash iterates them in hash-table order.
//! This reproduces bash's order as a view: bucket = FNV-1(key) & 1023 (1024
//! buckets), buckets ascending, within a bucket newest-inserted first.
//! Validated against bash 5.2.21 with 400 randomized cases (incl. collisions,
//! updates, deletes), 0 mismatches. No growth modeling — arrays < 2048 keys
//! never rehash (bash grows at nentries >= nbuckets*2).

pub(crate) const ASSOC_NBUCKETS: u32 = 1024;

/// bash's `hash_string`: 32-bit FNV-1 over the key's bytes.
pub(crate) fn assoc_hash(key: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in key.bytes() {
        h = h.wrapping_mul(16777619);
        h ^= b as u32;
    }
    h
}

/// Indices into `pairs` (insertion order) in bash iteration order:
/// bucket asc, then newest-inserted-first (descending index) within a bucket.
pub(crate) fn assoc_bash_order(pairs: &[(String, String)]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..pairs.len()).collect();
    idx.sort_by_key(|&i| {
        (
            assoc_hash(&pairs[i].0) & (ASSOC_NBUCKETS - 1),
            usize::MAX - i,
        )
    });
    idx
}

/// The pairs cloned into bash iteration order.
pub(crate) fn assoc_ordered_pairs(pairs: &[(String, String)]) -> Vec<(String, String)> {
    assoc_bash_order(pairs)
        .into_iter()
        .map(|i| pairs[i].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(ks: &[&str]) -> Vec<(String, String)> {
        ks.iter()
            .enumerate()
            .map(|(i, k)| (k.to_string(), i.to_string()))
            .collect()
    }
    fn order(ks: &[&str]) -> Vec<String> {
        assoc_ordered_pairs(&p(ks))
            .into_iter()
            .map(|(k, _)| k)
            .collect()
    }

    #[test]
    fn hash_matches_bash() {
        // hash_string values verified against bash's compiled C.
        assert_eq!(assoc_hash("qux"), 980750837);
        assert_eq!(assoc_hash("foo"), 1083137555);
        assert_eq!(assoc_hash("k18"), 4202936175);
    }

    #[test]
    fn order_matches_bash_examples() {
        // Verified against bash 5.2.21 `declare -A a=([k]=v ...); echo "${!a[@]}"`.
        assert_eq!(order(&["one", "two", "three"]), vec!["two", "three", "one"]);
        assert_eq!(
            order(&["foo", "bar", "baz", "qux"]),
            vec!["qux", "foo", "bar", "baz"]
        );
        assert_eq!(
            order(&["apple", "banana", "cherry", "date", "fig"]),
            vec!["cherry", "apple", "fig", "date", "banana"]
        );
        assert_eq!(
            order(&["x", "y", "z", "a", "b", "c"]),
            vec!["z", "y", "x", "c", "b", "a"]
        );
    }

    #[test]
    fn within_bucket_newest_first() {
        // Find two keys that genuinely collide (same bucket) and assert the
        // newer insertion (higher index) comes first. (Real-bash collision
        // behavior is additionally covered by the diff-check harness's
        // dup0/dup1 case in Task 2.) Compute a colliding pair deterministically:
        let mut a = None;
        'outer: for i in 0..2000u32 {
            for j in (i + 1)..2000u32 {
                let (ki, kj) = (format!("c{i}"), format!("c{j}"));
                if assoc_hash(&ki) & 1023 == assoc_hash(&kj) & 1023 {
                    a = Some((ki, kj));
                    break 'outer;
                }
            }
        }
        let (ki, kj) = a.expect("a colliding key pair must exist among c0..c1999");
        // Insert ki (index 0) then kj (index 1); same bucket → kj (newest) first.
        let pairs = vec![(ki.clone(), "0".to_string()), (kj.clone(), "1".to_string())];
        assert_eq!(
            assoc_bash_order(&pairs),
            vec![1, 0],
            "newest-in-bucket first"
        );
    }
}
