//! The native timestamp clustering decision over its at most two neighbors.
//! Complete and indexed Original owners supply the same ordered neighbors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Change {
    pub(crate) left: Option<(i64, i64)>,
    pub(crate) right: Option<(i64, i64)>,
    pub(crate) interval: (i64, i64),
    pub(crate) changed: bool,
}
pub(crate) fn cluster(left: Option<(i64, i64)>, right: Option<(i64, i64)>, time: i64) -> Change {
    let left = left.filter(|interval| {
        i128::from(time) - i128::from(interval.1) <= i128::from(super::super::BUSY_GAP_MS)
    });
    let right = right.filter(|interval| {
        i128::from(interval.0) - i128::from(time) <= i128::from(super::super::BUSY_GAP_MS)
    });
    let interval = (
        left.map_or(time, |interval| interval.0),
        right.map_or_else(
            || left.map_or(time, |interval| interval.1.max(time)),
            |interval| interval.1.max(time),
        ),
    );
    let remove = usize::from(left.is_some()) + usize::from(right.is_some());
    Change {
        left,
        right,
        interval,
        changed: remove != 1 || left.or(right) != Some(interval),
    }
}
pub(crate) fn bounds(first: Option<i64>, last: Option<i64>, time: i64) -> (i64, i64) {
    (
        first.map_or(time, |first| first.min(time)),
        last.map_or(time, |last| last.max(time)),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_neighbors_equal_independent_sorted_timestamp_clustering() {
        for points in [
            vec![],
            vec![0, 300_000, 600_000],
            vec![900_000, 0, 600_000, 300_000],
            vec![i64::MIN, i64::MAX, 0, 1, -1],
        ] {
            let mut actual = Vec::new();
            for &point in &points {
                crate::events::busy_add(&mut actual, point);
            }
            let mut sorted = points;
            sorted.sort_unstable();
            let mut expected: Vec<(i64, i64)> = Vec::new();
            for point in sorted {
                if let Some(last) = expected.last_mut()
                    && i128::from(point) - i128::from(last.1) <= 300_000
                {
                    last.1 = point;
                } else {
                    expected.push((point, point));
                }
            }
            assert_eq!(actual, expected);
        }
    }
}
