//! Native copy visibility over a common physical source modification cutoff.
//! The producer supplies only exact, eligible continued-in predecessors with
//! the same native copy identity. No UUID or source is inferred from a hash.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum Visibility {
    #[default]
    NoCopy,
    Modified(u128),
    /// The native scan retains a source whose modification clock is unknown.
    AlwaysCopied,
}
impl Visibility {
    pub(crate) fn observe(&mut self, predecessor_modified_ns: Option<u128>) {
        *self = match (*self, predecessor_modified_ns) {
            (Self::AlwaysCopied, _) | (_, None) => Self::AlwaysCopied,
            (Self::NoCopy, Some(time)) => Self::Modified(time),
            (Self::Modified(old), Some(time)) => Self::Modified(old.max(time)),
        };
    }
    pub(crate) fn visible(self, cutoff: Option<i64>) -> bool {
        match self {
            Self::NoCopy => true,
            Self::Modified(time) => !super::source_modified_in_window(Some(time), cutoff),
            Self::AlwaysCopied => false,
        }
    }
    /// Existing generic Order x9-byte geometry uses this exact scalar. The
    /// query's exclusive upper bound is upper(cutoff); no source bitmask or
    /// history-sized set of UUID conditions enters a foreground read.
    pub(crate) fn scalar(self) -> [u8; 9] {
        let mut key = [0; 9];
        match self {
            Self::NoCopy => {}
            Self::Modified(time) => {
                key[0] = 1;
                key[1..].copy_from_slice(
                    &u64::try_from(time / 1_000_000)
                        .unwrap_or(u64::MAX)
                        .to_be_bytes(),
                );
            }
            Self::AlwaysCopied => key[0] = 2,
        }
        key
    }
    pub(crate) fn upper(cutoff: Option<i64>) -> [u8; 9] {
        let mut key = [0; 9];
        key[0] = 1;
        key[1..].copy_from_slice(
            &cutoff
                .and_then(|time| u64::try_from(time).ok())
                .unwrap_or(0)
                .to_be_bytes(),
        );
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Event, Kind, native_event::NativeEvent};

    #[test]
    fn cutoff_visibility_matches_every_native_predecessor_and_scalar_rectangle() {
        let clocks = [
            Some(0),
            Some(999_999),
            Some(1_000_000),
            Some(7_000_000),
            Some(u128::MAX),
            None,
        ];
        for set in 0u32..1 << clocks.len() {
            let selected: Vec<_> = clocks
                .iter()
                .enumerate()
                .filter_map(|(bit, time)| (set & (1 << bit) != 0).then_some(*time))
                .collect();
            let mut visibility = Visibility::default();
            for time in &selected {
                visibility.observe(*time);
            }
            for cutoff in [
                None,
                Some(i64::MIN),
                Some(-1),
                Some(0),
                Some(1),
                Some(7),
                Some(8),
                Some(i64::MAX),
            ] {
                let expected = !selected
                    .iter()
                    .any(|time| crate::model::source_modified_in_window(*time, cutoff));
                assert_eq!(visibility.visible(cutoff), expected);
                assert_eq!(visibility.scalar() < Visibility::upper(cutoff), expected);
            }
        }
    }

    #[test]
    fn descriptor_copy_identity_matches_complete_native_choice_including_empty_uuid() {
        for kind in [Kind::U, Kind::A, Kind::Tool, Kind::Xsm] {
            for uuid in [None, Some(""), Some("same")] {
                for id in [None, Some(""), Some("same"), Some("different")] {
                    let event = Event {
                        k: kind,
                        u: uuid.map(str::to_owned),
                        id: id.map(str::to_owned),
                        ..Event::default()
                    };
                    let native: NativeEvent<String> = event.clone().into();
                    assert_eq!(
                        native.copy_identity().cloned(),
                        crate::model::native_copy_identity(&event)
                    );
                }
            }
        }
    }
}
