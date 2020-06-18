use super::range_wrapper::StartWrapper;
use crate::std_ext::*;
use std::collections::BTreeMap;
use std::ops::Range;

#[derive(Clone)]
/// A map whose keys are stored as ranges.
///
/// Contiguous and overlapping ranges that map to the same value
/// are coalesced into a single range.
pub struct RangeMap<K, V> {
    // Wrap ranges so that they are `Ord`.
    // See `range_wrapper.rs` for explanation.
    btm: BTreeMap<StartWrapper<K>, V>,
}

impl<K, V> Default for RangeMap<K, V>
where
    K: Ord + Clone,
    V: Eq + Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> RangeMap<K, V>
where
    K: Ord + Clone,
    V: Eq + Clone,
{
    /// Makes a new empty `RangeMap`.
    pub fn new() -> Self {
        RangeMap {
            btm: BTreeMap::new(),
        }
    }

    /// Returns a reference to the value corresponding to the given key,
    /// if the key is covered by any range in the map.
    pub fn get(&self, key: &K) -> Option<&V> {
        self.get_key_value(key).map(|(_range, value)| value)
    }

    /// Returns the range-value pair (as a pair of references) corresponding
    /// to the given key, if the key is covered by any range in the map.
    pub fn get_key_value(&self, key: &K) -> Option<(&Range<K>, &V)> {
        use std::ops::Bound;

        // The only stored range that could contain the given key is the
        // last stored range whose start is less than or equal to this key.
        let key_as_start = StartWrapper::new(key.clone()..key.clone());
        self.btm
            .range((Bound::Unbounded, Bound::Included(key_as_start)))
            .next_back()
            .filter(|(range_start_wrapper, _value)| {
                // Does the only candidate range contain
                // the requested key?
                //
                // TODO: Use `contains` once https://github.com/rust-lang/rust/issues/32311
                // is stabilized.
                range_start_wrapper.range.contains_item(key)
            })
            .map(|(range_start_wrapper, value)| (&range_start_wrapper.range, value))
    }

    /// Returns `true` if any range in the map covers the specified key.
    pub fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    /// Gets an iterator over all pairs of key range and value,
    /// ordered by key range.
    ///
    /// The iterator element type is `(&'a Range<K>, &'a V)`.
    pub fn iter(&self) -> impl Iterator<Item = (&Range<K>, &V)> {
        self.btm.iter().map(|(by_start, v)| (&by_start.range, v))
    }

    /// Insert a pair of key range and value into the map.
    ///
    /// If the inserted range partially or completely overlaps any
    /// existing range in the map, then the existing range (or ranges) will be
    /// partially or completely replaced by the inserted range.
    ///
    /// If the inserted range either overlaps or is immediately adjacent
    /// any existing range _mapping to the same value_, then the ranges
    /// will be coalesced into a single contiguous range.
    ///
    /// # Panics
    ///
    /// Panics if range `start > end`.
    pub fn insert(&mut self, range: Range<K>, value: V) {
        use std::ops::Bound;

        // We don't want to have to make empty ranges make sense;
        // they don't represent anything meaningful in this structure.
        assert!(range.start <= range.end);

        // Wrap up the given range so that we can "borrow"
        // it as a wrapper reference to either its start or end.
        // See `range_wrapper.rs` for explanation of these hacks.
        let mut new_range_start_wrapper: StartWrapper<K> = StartWrapper::new(range);
        let new_value = value;

        // Is there a stored range either overlapping the start of
        // the range to insert or immediately preceding it?
        //
        // If there is any such stored range, it will be the last
        // whose start is less than or equal to the start of the range to insert.
        if let Some((stored_range_start_wrapper, stored_value)) = self
            .btm
            .range((Bound::Unbounded, Bound::Included(&new_range_start_wrapper)))
            .next_back()
            .filter(|(stored_range_start_wrapper, _stored_value)| {
                // Does the only candidate range either overlap
                // or immediately precede the range to insert?
                // (Remember that it might actually cover the _whole_
                // range to insert and then some.)
                stored_range_start_wrapper
                    .range
                    .touches(&new_range_start_wrapper.range)
            })
            .map(|(stored_range_start_wrapper, stored_value)| {
                (stored_range_start_wrapper.clone(), stored_value.clone())
            })
        {
            self.adjust_touching_ranges_for_insert(
                stored_range_start_wrapper,
                stored_value,
                &mut new_range_start_wrapper.range,
                &new_value,
            );
        }

        // Are there any stored ranges whose heads overlap or immediately
        // follow the range to insert?
        //
        // If there are any such stored ranges (that weren't already caught above),
        // their starts will fall somewhere after the start of the range to insert,
        // and on or before its end.
        //
        // This time around, if the latter holds, it also implies
        // the former so we don't need to check here if they touch.
        //
        // REVISIT: Possible micro-optimisation: `impl Borrow<T> for StartWrapper<T>`
        // and use that to search here, to avoid constructing another `StartWrapper`.
        let new_range_end_as_start = StartWrapper::new(
            new_range_start_wrapper.range.end.clone()..new_range_start_wrapper.range.end.clone(),
        );
        while let Some((stored_range_start_wrapper, stored_value)) = self
            .btm
            .range((
                Bound::Excluded(&new_range_start_wrapper),
                Bound::Included(&new_range_end_as_start),
            ))
            .next()
        {
            // One extra exception: if we have different values,
            // and the stored range starts at the end of the range to insert,
            // then we don't want to keep looping forever trying to find more!
            if stored_range_start_wrapper.range.start == new_range_start_wrapper.range.end
                && *stored_value != new_value
            {
                break;
            }

            let stored_range_start_wrapper = stored_range_start_wrapper.clone();
            let stored_value = stored_value.clone();

            self.adjust_touching_ranges_for_insert(
                stored_range_start_wrapper,
                stored_value,
                &mut new_range_start_wrapper.range,
                &new_value,
            );
        }

        // Insert the (possibly expanded) new range, and we're done!
        self.btm.insert(new_range_start_wrapper, new_value);
    }

    /// Removes a range from the map, if all or any of it was present.
    ///
    /// If the range to be removed _partially_ overlaps any ranges
    /// in the map, then those ranges will be contracted to no
    /// longer cover the removed range.
    ///
    ///
    /// # Panics
    ///
    /// Panics if range `start >= end`.
    pub fn remove(&mut self, range: Range<K>) {
        use std::ops::Bound;

        // We don't want to have to make empty ranges make sense;
        // they don't represent anything meaningful in this structure.
        assert!(range.start < range.end);

        let range_start_wrapper: StartWrapper<K> = StartWrapper::new(range);
        let range = &range_start_wrapper.range;

        // Is there a stored range overlapping the start of
        // the range to insert?
        //
        // If there is any such stored range, it will be the last
        // whose start is less than or equal to the start of the range to insert.
        if let Some((stored_range_start_wrapper, stored_value)) = self
            .btm
            .range((Bound::Unbounded, Bound::Included(&range_start_wrapper)))
            .next_back()
            .filter(|(stored_range_start_wrapper, _stored_value)| {
                // Does the only candidate range overlap
                // the range to insert?
                stored_range_start_wrapper.range.overlaps(&range)
            })
            .map(|(stored_range_start_wrapper, stored_value)| {
                (stored_range_start_wrapper.clone(), stored_value.clone())
            })
        {
            self.adjust_overlapping_ranges_for_remove(
                stored_range_start_wrapper,
                stored_value,
                &range,
            );
        }

        // Are there any stored ranges whose heads overlap the range to insert?
        //
        // If there are any such stored ranges (that weren't already caught above),
        // their starts will fall somewhere after the start of the range to insert,
        // and before its end.
        //
        // REVISIT: Possible micro-optimisation: `impl Borrow<T> for StartWrapper<T>`
        // and use that to search here, to avoid constructing another `StartWrapper`.
        let new_range_end_as_start = StartWrapper::new(range.end.clone()..range.end.clone());
        while let Some((stored_range_start_wrapper, stored_value)) = self
            .btm
            .range((
                Bound::Excluded(&range_start_wrapper),
                Bound::Excluded(&new_range_end_as_start),
            ))
            .next()
            .map(|(stored_range_start_wrapper, stored_value)| {
                (stored_range_start_wrapper.clone(), stored_value.clone())
            })
        {
            self.adjust_overlapping_ranges_for_remove(
                stored_range_start_wrapper,
                stored_value,
                &range,
            );
        }
    }

    fn adjust_touching_ranges_for_insert(
        &mut self,
        stored_range_start_wrapper: StartWrapper<K>,
        stored_value: V,
        new_range: &mut Range<K>,
        new_value: &V,
    ) {
        use std::cmp::{max, min};

        if stored_value == *new_value {
            // The ranges have the same value, so we can "adopt"
            // the stored range.
            //
            // This means that no matter how big or where the stored range is,
            // we will expand the new range's bounds to subsume it,
            // and then delete the stored range.
            new_range.start =
                min(&new_range.start, &stored_range_start_wrapper.range.start).clone();
            new_range.end = max(&new_range.end, &stored_range_start_wrapper.range.end).clone();
            self.btm.remove(&stored_range_start_wrapper);
        } else {
            // The ranges have different values.
            if new_range.overlaps(&stored_range_start_wrapper.range) {
                // The ranges overlap. This is a little bit more complicated.
                // Delete the stored range, and then add back between
                // 0 and 2 subranges at the ends of the range to insert.
                self.btm.remove(&stored_range_start_wrapper);
                if stored_range_start_wrapper.range.start < new_range.start {
                    // Insert the piece left of the range to insert.
                    self.btm.insert(
                        StartWrapper::new(
                            stored_range_start_wrapper.range.start..new_range.start.clone(),
                        ),
                        stored_value.clone(),
                    );
                }
                if stored_range_start_wrapper.range.end > new_range.end {
                    // Insert the piece right of the range to insert.
                    self.btm.insert(
                        StartWrapper::new(
                            new_range.end.clone()..stored_range_start_wrapper.range.end,
                        ),
                        stored_value,
                    );
                }
            } else {
                // No-op; they're not overlapping,
                // so we can just keep both ranges as they are.
            }
        }
    }

    fn adjust_overlapping_ranges_for_remove(
        &mut self,
        stored_range_start_wrapper: StartWrapper<K>,
        stored_value: V,
        range_to_remove: &Range<K>,
    ) {
        // Delete the stored range, and then add back between
        // 0 and 2 subranges at the ends of the range to insert.
        self.btm.remove(&stored_range_start_wrapper);
        let stored_range = stored_range_start_wrapper.range;
        if stored_range.start < range_to_remove.start {
            // Insert the piece left of the range to insert.
            self.btm.insert(
                StartWrapper::new(stored_range.start..range_to_remove.start.clone()),
                stored_value.clone(),
            );
        }
        if stored_range.end > range_to_remove.end {
            // Insert the piece right of the range to insert.
            self.btm.insert(
                StartWrapper::new(range_to_remove.end.clone()..stored_range.end),
                stored_value,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    trait RangeMapExt<K, V> {
        fn to_vec(&self) -> Vec<(Range<K>, V)>;
    }

    impl<K, V> RangeMapExt<K, V> for RangeMap<K, V>
    where
        K: Ord + Clone,
        V: Eq + Clone,
    {
        fn to_vec(&self) -> Vec<(Range<K>, V)> {
            self.iter().map(|(kr, v)| (kr.clone(), v.clone())).collect()
        }
    }

    //
    // Insertion tests
    //

    #[test]
    fn empty_map_is_empty() {
        let range_map: RangeMap<u32, bool> = RangeMap::new();
        assert_eq!(range_map.to_vec(), vec![]);
    }

    #[test]
    fn insert_into_empty_map() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        range_map.insert(0..50, false);
        assert_eq!(range_map.to_vec(), vec![(0..50, false)]);
    }

    #[test]
    fn new_same_value_immediately_following_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●---◌ ◌ ◌ ◌ ◌ ◌ ◌
        range_map.insert(1..3, false);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ◌ ◌ ●---◌ ◌ ◌ ◌ ◌
        range_map.insert(3..5, false);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●-------◌ ◌ ◌ ◌ ◌
        assert_eq!(range_map.to_vec(), vec![(1..5, false)]);
    }

    #[test]
    fn new_different_value_immediately_following_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●---◌ ◌ ◌ ◌ ◌ ◌ ◌
        range_map.insert(1..3, false);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ◌ ◌ ◆---◇ ◌ ◌ ◌ ◌
        range_map.insert(3..5, true);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●---◌ ◌ ◌ ◌ ◌ ◌ ◌
        // ◌ ◌ ◌ ◆---◇ ◌ ◌ ◌ ◌
        assert_eq!(range_map.to_vec(), vec![(1..3, false), (3..5, true)]);
    }

    #[test]
    fn new_same_value_overlapping_end_of_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●-----◌ ◌ ◌ ◌ ◌ ◌
        range_map.insert(1..4, false);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ◌ ◌ ●---◌ ◌ ◌ ◌ ◌
        range_map.insert(3..5, false);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●-------◌ ◌ ◌ ◌ ◌
        assert_eq!(range_map.to_vec(), vec![(1..5, false)]);
    }

    #[test]
    fn new_different_value_overlapping_end_of_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●-----◌ ◌ ◌ ◌ ◌ ◌
        range_map.insert(1..4, false);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ◌ ◌ ◆---◇ ◌ ◌ ◌ ◌
        range_map.insert(3..5, true);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●---◌ ◌ ◌ ◌ ◌ ◌ ◌
        // ◌ ◌ ◌ ◆---◇ ◌ ◌ ◌ ◌
        assert_eq!(range_map.to_vec(), vec![(1..3, false), (3..5, true)]);
    }

    #[test]
    fn new_same_value_immediately_preceding_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ◌ ◌ ●---◌ ◌ ◌ ◌ ◌
        range_map.insert(3..5, false);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●---◌ ◌ ◌ ◌ ◌ ◌ ◌
        range_map.insert(1..3, false);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●-------◌ ◌ ◌ ◌ ◌
        assert_eq!(range_map.to_vec(), vec![(1..5, false)]);
    }

    #[test]
    fn new_different_value_immediately_preceding_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ◌ ◌ ◆---◇ ◌ ◌ ◌ ◌
        range_map.insert(3..5, true);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●---◌ ◌ ◌ ◌ ◌ ◌ ◌
        range_map.insert(1..3, false);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●---◌ ◌ ◌ ◌ ◌ ◌ ◌
        // ◌ ◌ ◌ ◆---◇ ◌ ◌ ◌ ◌
        assert_eq!(range_map.to_vec(), vec![(1..3, false), (3..5, true)]);
    }

    #[test]
    fn new_same_value_wholly_inside_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●-------◌ ◌ ◌ ◌ ◌
        range_map.insert(1..5, false);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ◌ ●---◌ ◌ ◌ ◌ ◌ ◌ ◌
        range_map.insert(2..4, false);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●-------◌ ◌ ◌ ◌ ◌
        assert_eq!(range_map.to_vec(), vec![(1..5, false)]);
    }

    #[test]
    fn new_different_value_wholly_inside_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ◆-------◇ ◌ ◌ ◌ ◌
        range_map.insert(1..5, true);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ◌ ●---◌ ◌ ◌ ◌ ◌ ◌ ◌
        range_map.insert(2..4, false);
        // 0 1 2 3 4 5 6 7 8 9
        // ◌ ●-◌ ◌ ◌ ◌ ◌ ◌ ◌ ◌
        // ◌ ◌ ◆---◇ ◌ ◌ ◌ ◌ ◌
        // ◌ ◌ ◌ ◌ ●-◌ ◌ ◌ ◌ ◌
        assert_eq!(
            range_map.to_vec(),
            vec![(1..2, true), (2..4, false), (4..5, true)]
        );
    }

    #[test]
    // Test every permutation of a bunch of touching and overlapping ranges.
    fn lots_of_interesting_ranges() {
        use crate::stupid_range_map::StupidU32RangeMap;
        use permutator::Permutation;

        let mut ranges_with_values = [
            (2..3, false),
            // A duplicate duplicates
            (2..3, false),
            // Almost a duplicate, but with a different value
            (2..3, true),
            // A few small ranges, some of them overlapping others,
            // some of them touching others
            (3..5, true),
            (4..6, true),
            (5..7, true),
            // A really big range
            (2..6, true),
        ];

        ranges_with_values.permutation().for_each(|permutation| {
            let mut range_map: RangeMap<u32, bool> = RangeMap::new();
            let mut stupid: StupidU32RangeMap<bool> = StupidU32RangeMap::new();

            for (k, v) in permutation {
                // Insert it into both maps.
                range_map.insert(k.clone(), v);
                stupid.insert(k, v);

                // At every step, both maps should contain the same stuff.
                let stupid2: StupidU32RangeMap<bool> = range_map.clone().into();
                assert_eq!(stupid, stupid2);
            }
        });
    }

    //
    // Get* tests
    //

    #[test]
    fn get() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        range_map.insert(0..50, false);
        assert_eq!(range_map.get(&49), Some(&false));
        assert_eq!(range_map.get(&50), None);
    }

    #[test]
    fn get_key_value() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        range_map.insert(0..50, false);
        assert_eq!(range_map.get_key_value(&49), Some((&(0..50), &false)));
        assert_eq!(range_map.get_key_value(&50), None);
    }

    //
    // Removal tests
    //

    #[test]
    fn remove_from_empty_map() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        range_map.remove(0..50);
        assert_eq!(range_map.to_vec(), vec![]);
    }

    #[test]
    fn remove_non_covered_range_before_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        range_map.insert(25..75, false);
        range_map.remove(0..25);
        assert_eq!(range_map.to_vec(), vec![(25..75, false)]);
    }

    #[test]
    fn remove_non_covered_range_after_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        range_map.insert(25..75, false);
        range_map.remove(75..100);
        assert_eq!(range_map.to_vec(), vec![(25..75, false)]);
    }

    #[test]
    fn remove_overlapping_start_of_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        range_map.insert(25..75, false);
        range_map.remove(0..30);
        assert_eq!(range_map.to_vec(), vec![(30..75, false)]);
    }

    #[test]
    fn remove_middle_of_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        range_map.insert(25..75, false);
        range_map.remove(30..70);
        assert_eq!(range_map.to_vec(), vec![(25..30, false), (70..75, false)]);
    }

    #[test]
    fn remove_overlapping_end_of_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        range_map.insert(25..75, false);
        range_map.remove(70..100);
        assert_eq!(range_map.to_vec(), vec![(25..70, false)]);
    }

    #[test]
    fn remove_exactly_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        range_map.insert(25..75, false);
        range_map.remove(25..75);
        assert_eq!(range_map.to_vec(), vec![]);
    }

    #[test]
    fn remove_superset_of_stored() {
        let mut range_map: RangeMap<u32, bool> = RangeMap::new();
        range_map.insert(25..75, false);
        range_map.remove(0..100);
        assert_eq!(range_map.to_vec(), vec![]);
    }
}
