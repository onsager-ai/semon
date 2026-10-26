//! Ownership decisions from the ordinary semantic staging producer. Adapters
//! read qualified state and persist the result in their surrounding transaction.
use super::StorageError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Begin {
    Create,
    Reset,
    Resume,
    Continue,
}

pub(crate) struct Observed<'a> {
    pub(crate) epoch: Option<i64>,
    pub(crate) runtime: Option<&'a str>,
    pub(crate) generation: Option<&'a str>,
    pub(crate) invalid_withdrawal: bool,
}

pub(crate) fn bound_runtime(
    namespace: &str,
    admission: &impl serde::Serialize,
    runtime: &str,
) -> Result<String, StorageError> {
    if runtime.len() > 65536 {
        return Err(StorageError::InvalidArguments);
    }
    serde_json::to_string(&(namespace, admission, runtime)).map_err(|_| StorageError::Unavailable)
}

pub(crate) fn begin(
    observed: Observed<'_>,
    held_epoch: Option<i64>,
    begun: bool,
    generation: Option<&str>,
    runtime: &str,
    reduction: bool,
) -> Result<Begin, StorageError> {
    if held_epoch.is_some() && observed.epoch != held_epoch {
        return Err(StorageError::Conflict);
    }
    let Some(old_runtime) = observed.runtime else {
        return Ok(Begin::Create);
    };
    if observed.generation != generation {
        return Err(StorageError::Conflict);
    }
    if observed.invalid_withdrawal || old_runtime != runtime {
        return if reduction {
            Ok(Begin::Reset)
        } else {
            Err(StorageError::NamespaceUpgradePending)
        };
    }
    Ok(if !begun || held_epoch.is_none() {
        Begin::Resume
    } else {
        Begin::Continue
    })
}

#[cfg(test)]
#[test]
fn catalog138_owner_transition_fences_successors_and_resumes_exact_runtime() {
    let state = || Observed {
        epoch: Some(4),
        runtime: Some("runtime"),
        generation: Some("head"),
        invalid_withdrawal: false,
    };
    assert_eq!(
        begin(state(), Some(3), true, Some("head"), "runtime", true),
        Err(StorageError::Conflict)
    );
    assert_eq!(
        begin(state(), None, false, Some("head"), "runtime", true),
        Ok(Begin::Resume)
    );
    assert_eq!(
        begin(state(), Some(4), true, Some("head"), "runtime", true),
        Ok(Begin::Continue)
    );
    assert_eq!(
        begin(state(), Some(4), true, Some("other"), "runtime", true),
        Err(StorageError::Conflict)
    );
    assert_eq!(
        begin(state(), Some(4), true, Some("head"), "new", false),
        Err(StorageError::NamespaceUpgradePending)
    );
    assert_eq!(
        begin(state(), Some(4), true, Some("head"), "new", true),
        Ok(Begin::Reset)
    );
    assert_ne!(
        bound_runtime("a", &1u64, "r").unwrap(),
        bound_runtime("a", &2u64, "r").unwrap()
    );
}
