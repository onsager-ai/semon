//! Current reported-fact row contract. Contains only the producer's allowlist,
//! never project paths, prompt text or tool bodies. Both adapters use this codec.
use super::ReportedFileStamp;
use crate::facts::ReportedRunSnapshot;
#[cfg(any(test, feature = "postgres"))]
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(any(test, feature = "postgres"))]
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    run: ReportedRunSnapshot,
}

#[derive(Clone)]
pub(crate) struct ReportedRow {
    pub(crate) run: ReportedRunSnapshot,
    pub(crate) model_usage: String,
}

impl ReportedRow {
    pub(crate) fn new(run: &ReportedRunSnapshot) -> Result<Self, serde_json::Error> {
        if run.last_cost.is_some_and(|v| !v.is_finite())
            || run
                .last_model_usage
                .values()
                .any(|v| v.cost_usd.is_some_and(|v| !v.is_finite()))
        {
            return Err(invalid("nonfinite reported cost"));
        }
        Ok(Self {
            run: run.clone(),
            model_usage: serde_json::to_string(&run.last_model_usage)?,
        })
    }

    #[cfg(any(test, feature = "postgres"))]
    pub(crate) fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&Record {
            version: 1,
            run: self.run.clone(),
        })
    }

    #[cfg(any(test, feature = "postgres"))]
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let record: Record = serde_json::from_slice(bytes)?;
        if record.version != 1 {
            return Err(invalid("unsupported reported record"));
        }
        let row = Self::new(&record.run)?;
        if row.encode()? != bytes {
            return Err(invalid("noncanonical or incomplete reported record"));
        }
        Ok(row)
    }
}

fn invalid(message: &str) -> serde_json::Error {
    <serde_json::Error as serde::de::Error>::custom(message)
}

/// The production merge key: a later observation replaces only the same run.
/// Other starts, including observations no longer present in the input, survive.
pub(crate) fn plan(runs: &[ReportedRunSnapshot]) -> Result<Vec<ReportedRow>, serde_json::Error> {
    let mut rows = BTreeMap::new();
    for run in runs {
        rows.insert(
            (run.last_session_id.clone(), run.last_start_time),
            ReportedRow::new(run)?,
        );
    }
    Ok(rows.into_values().collect())
}

pub(crate) fn encode_stamp(stamp: &ReportedFileStamp) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(stamp)
}

pub(crate) fn decode_stamp(bytes: &[u8]) -> Result<ReportedFileStamp, serde_json::Error> {
    serde_json::from_slice(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reported_rows_lossless_current_codec_and_merge() {
        let mut a = ReportedRunSnapshot {
            last_session_id: "同\0native".into(),
            last_start_time: i64::MIN,
            last_duration: Some(u64::MAX),
            capture_at: i64::MAX,
            ..Default::default()
        };
        a.last_model_usage.insert(
            "模型\0".into(),
            crate::facts::ReportedModelUsage {
                input_tokens: u64::MAX,
                ..Default::default()
            },
        );
        let mut b = a.clone();
        b.last_start_time += 1;
        let mut replacement = a.clone();
        replacement.last_cost = Some(0.125);
        let rows = plan(&[a, b.clone(), replacement.clone()]).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            ReportedRow::decode(&rows[0].encode().unwrap()).unwrap().run,
            replacement
        );
        assert_eq!(
            ReportedRow::decode(&rows[1].encode().unwrap()).unwrap().run,
            b
        );
        assert!(ReportedRow::decode(br#"{"version":2,"run":{}}"#).is_err());
        assert!(ReportedRow::decode(b"broken").is_err());
        let bad = ReportedRunSnapshot {
            last_cost: Some(f64::NAN),
            ..Default::default()
        };
        assert!(plan(&[bad]).is_err());
    }
}
