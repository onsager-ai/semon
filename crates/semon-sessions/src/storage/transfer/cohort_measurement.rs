//! Test-only fixed-family sharing experiment. No durable format is activated.
use super::*;
use crate::{model::{PreparedSources, relation_stage::Batch}, storage::proof_codec};
use serde_json::{Value, json};
use std::collections::BTreeMap;

type Records = Vec<(String, String, Value)>;

#[derive(Clone, Serialize, Deserialize)]
enum Fact {
    Exact(Value),
    Cell(String),
    Catalog(String),
    Relative { catalog: String, lifecycle: Value, harness: Value },
    Details { owner: String, live: Value },
    Trace { owner: String, id: String, edge: bool },
}

fn invalid() -> StorageError { StorageError::ScopeChanged }
fn get<'a>(records: &'a Records, family: &str, key: &str) -> Result<&'a Value,StorageError> {
    records.iter().find(|(f,k,_)| f == family && k == key).map(|(_,_,v)|v).ok_or_else(invalid)
}
fn resolve(fact: &Fact, records: &Records, catalogs: &BTreeMap<String,Fact>) -> Result<Value,StorageError> {
    Ok(match fact {
        Fact::Exact(value) => value.clone(),
        Fact::Cell(owner) => get(records,"cell",owner)?[0].clone(),
        Fact::Catalog(key) => resolve(catalogs.get(key).ok_or_else(invalid)?,records,catalogs)?,
        Fact::Details { owner, live } => {
            let session = crate::model::typed_witness::cohort_session(records,owner).map_err(|_|invalid())?;
            let mut details = session["details"].clone();
            details["busy"] = session["busy"].clone();
            details["content_generation"] = session["content_generation"].clone();
            json!([serde_json::to_string(&details).map_err(|_|invalid())?,live])
        }
        Fact::Trace { owner,id,edge } => {
            let trace = get(records,"trace",owner)?;
            let values = trace[if *edge { "edges" } else { "turns" }].as_array().ok_or_else(invalid)?;
            let value = values.iter().find(|v| v["id"].as_str()==Some(id)).ok_or_else(invalid)?;
            json!(serde_json::to_string(value).map_err(|_|invalid())?)
        }
        Fact::Relative { catalog,lifecycle,harness } => {
            let text = resolve(catalogs.get(catalog).ok_or_else(invalid)?,records,catalogs)?;
            let mut metadata: Value = serde_json::from_str(text.as_str().ok_or_else(invalid)?).map_err(|_|invalid())?;
            metadata["lifecycle"] = lifecycle.clone();
            let kind = if metadata["presentation_kind"].is_null() { &metadata["kind"] } else { &metadata["presentation_kind"] };
            let value = json!({"key":metadata["key"],"name":metadata["name"],"harness":harness,"kind":kind,"parent":metadata["parent"],"last":metadata["last"],"cost":metadata["cost"],"_status_catalog":metadata});
            json!(serde_json::to_string(&value).map_err(|_|invalid())?)
        }
    })
}

pub(crate) fn qualify(
    config: &StoreConfig, batch: &Batch, prepared: &PreparedSources,
    rows: &[(String,String)], manifest: &str, base: &str,
) -> Result<(),StorageError> {
    let mut records = crate::model::typed_witness::cohort_records(prepared).map_err(|error| {
        eprintln!("typed_cohort_refusal boundary=semantic reason={error}"); invalid()
    })?;
    let mut catalogs = BTreeMap::<String,Fact>::new();
    let mut catalog_by_owner = BTreeMap::<String,String>::new();
    let captures = rows.iter().map(|(owner,text)| {
        Ok((owner.clone(),serde_json::from_str::<Value>(text).map_err(|_|invalid())?))
    }).collect::<Result<Vec<_>,StorageError>>()?;
    let history = captures.iter().map(|(owner,value)| (owner.clone(),value["has_history"].as_bool().unwrap_or(false))).collect::<BTreeMap<_,_>>();
    for (_,capture) in &captures {
        for (args,value) in capture["context"]["preview_catalog"].as_object().ok_or_else(invalid)? {
            if value.is_null() { continue; }
            let args: Vec<String> = serde_json::from_str(args).map_err(|_|invalid())?;
            let owner = args.first().ok_or_else(invalid)?;
            let metadata: Value = serde_json::from_str(value.as_str().ok_or_else(invalid)?).map_err(|_|invalid())?;
            if metadata["key"].as_str()!=Some(owner) { return Err(invalid()); }
            let sources: Vec<crate::model::summary::CatalogSource> = serde_json::from_value(metadata["sources"].clone()).map_err(|_|invalid())?;
            let prefix = crate::retention::source_generation(&sources).map_err(|_|invalid())?;
            let residence = if *history.get(owner).ok_or_else(invalid)? { "history_context" } else { "current" };
            let key = serde_json::to_string(&(residence,owner,prefix)).map_err(|_|invalid())?;
            let fact = if residence=="current" {
                if get(&records,"cell",owner)?[0]!=*value { return Err(invalid()); }
                Fact::Cell(owner.clone())
            } else { Fact::Exact(value.clone()) };
            if let Some(previous) = catalogs.get(&key) {
                if serde_json::to_vec(&resolve(previous,&records,&catalogs)?).map_err(|_|invalid())? != serde_json::to_vec(value).map_err(|_|invalid())? { return Err(invalid()); }
            } else { catalogs.insert(key.clone(),fact); }
            if catalog_by_owner.insert(owner.clone(),key.clone()).is_some_and(|old| old!=key) { return Err(invalid()); }
        }
    }
    let mut exact = BTreeMap::<(String,String),Value>::new();
    let mut facts = BTreeMap::<(String,String),Fact>::new();
    let mut occurrences = BTreeMap::<String,usize>::new();
    for (owner,capture) in &captures {
        let mut header = capture.clone();
        let mut membership = Vec::new();
        for family in ["details","relative","descendants","header","turn","edge","target_turns","participant","preview_header","preview_catalog","relative_statuses"] {
            let values = if family=="relative_statuses" { &capture[family] } else { &capture["context"][family] };
            for (args,value) in values.as_object().ok_or_else(invalid)? {
                *occurrences.entry(family.into()).or_default() += 1;
                let coordinate = (family.to_owned(),args.clone());
                if let Some(previous) = exact.get(&coordinate) {
                    if serde_json::to_vec(previous).map_err(|_|invalid())? != serde_json::to_vec(value).map_err(|_|invalid())? {
                        eprintln!("typed_cohort_refusal boundary=context family={family} reason=conflicting_exact_observation");
                        return Err(invalid());
                    }
                } else {
                    let arguments: Vec<String> = if matches!(family,"preview_catalog" | "relative" | "details" | "turn" | "edge") {
                        serde_json::from_str(args).map_err(|_|invalid())?
                    } else { Vec::new() };
                    let target = || arguments.first().ok_or_else(invalid);
                    let fact = match family {
                        "preview_catalog" if !value.is_null() => Fact::Catalog(catalog_by_owner.get(target()?).ok_or_else(invalid)?.clone()),
                        "relative" if !value.is_null() => {
                            let relative: Value = serde_json::from_str(value.as_str().ok_or_else(invalid)?).map_err(|_|invalid())?;
                            Fact::Relative { catalog:catalog_by_owner.get(target()?).ok_or_else(invalid)?.clone(), lifecycle:relative["_status_catalog"]["lifecycle"].clone(), harness:relative["harness"].clone() }
                        }
                        "details" if !value.is_null() && value[0].is_string() => Fact::Details { owner:target()?.clone(),live:value[1].clone() },
                        "turn" | "edge" if value.is_string() => Fact::Trace { owner:target()?.clone(),id:arguments.get(1).ok_or_else(invalid)?.clone(),edge:family=="edge" },
                        _ => Fact::Exact(value.clone()),
                    };
                    if serde_json::to_vec(&resolve(&fact,&records,&catalogs)?).map_err(|_|invalid())? != serde_json::to_vec(value).map_err(|_|invalid())? {
                        eprintln!("typed_cohort_refusal boundary=fixed_projection family={family} reason=exact_bytes_differ");
                        return Err(invalid());
                    }
                    exact.insert(coordinate.clone(),value.clone());
                    facts.insert(coordinate.clone(),fact);
                }
                membership.push(coordinate);
            }
            if family=="relative_statuses" { header[family]=json!({}); } else { header["context"][family]=json!({}); }
        }
        records.push(("presentation".into(),owner.clone(),json!([header,membership])));
    }
    for (key,fact) in &catalogs { records.push(("context_catalog".into(),key.clone(),serde_json::to_value(fact).map_err(|_|invalid())?)); }
    for ((family,args),fact) in &facts {
        records.push((format!("context_{family}"),args.clone(),serde_json::to_value(fact).map_err(|_|invalid())?));
    }
    let control = json!({"namespace":config.namespace.as_str(),"stream_epoch":config.expected_stream_epoch,"epoch":batch.epoch,"phase":batch.phase,"paths":batch.paths,"base_generation":batch.base_generation,"revision":batch.source_revision,"runtime":batch.runtime,"context":batch.context,"relation_cursor":batch.relation_cursor,"claims":prepared.claims});
    records.push(("control".into(),String::new(),control));
    let root = proof_codec::presentation_digest(base,manifest)?;
    records.push(("immutable_root".into(),String::new(),json!([base,manifest,root])));
    let mut family_bytes = BTreeMap::<String,(usize,usize,usize)>::new();
    let mut total = 0usize;
    let mut decoded = Vec::new();
    for (family,key,value) in &records {
        let bytes = encode_bounded(&("typed-cohort-v1",family,key,value),MAX_PAGE_BYTES)?;
        let size = family_bytes.entry(family.clone()).or_default();
        size.0 += 1; size.1 += bytes.len(); size.2 = size.2.max(bytes.len()); total += bytes.len();
        let (codec,family,key,value): (String,String,String,Value) = serde_json::from_slice(&bytes).map_err(|_|invalid())?;
        if codec!="typed-cohort-v1" || encode_bounded(&(&codec,&family,&key,&value),MAX_PAGE_BYTES)? != bytes { return Err(invalid()); }
        decoded.push((family,key,value));
    }
    let catalogs = decoded.iter().filter(|(f,_,_)|f=="context_catalog").map(|(_,k,v)| Ok((k.clone(),serde_json::from_value::<Fact>(v.clone()).map_err(|_|invalid())?))).collect::<Result<BTreeMap<_,_>,StorageError>>()?;
    let mut reconstructed_bytes = 0usize;
    for (owner,original) in rows {
        let stored = get(&decoded,"presentation",owner)?;
        let mut capture = stored[0].clone();
        for membership in stored[1].as_array().ok_or_else(invalid)? {
            let family = membership[0].as_str().ok_or_else(invalid)?;
            let args = membership[1].as_str().ok_or_else(invalid)?;
            let fact: Fact = serde_json::from_value(get(&decoded,&format!("context_{family}"),args)?.clone()).map_err(|_|invalid())?;
            let value = resolve(&fact,&decoded,&catalogs)?;
            let target = if family=="relative_statuses" { &mut capture[family] } else { &mut capture["context"][family] };
            target.as_object_mut().ok_or_else(invalid)?.insert(args.into(),value);
        }
        let text = serde_json::to_string(&capture).map_err(|_|invalid())?;
        if text!=*original { return Err(invalid()); }
        reconstructed_bytes += text.len();
        let row = prepared.rows.iter().find(|row| &row.key==owner).ok_or_else(invalid)?;
        let metadata = crate::query_projection::SelectedRecipe::prepare(row).map_err(|_|invalid())?.metadata;
        let slots = serde_json::to_string(&prepared.transcripts.iter().find(|p| &p.key==owner).map(|p| &p.slots)).map_err(|_|invalid())?;
        proof_codec::verify_presentation_entry(owner,&metadata,&slots,&text,manifest)?;
    }
    for (family,(count,bytes,largest)) in &family_bytes {
        let observations = family.strip_prefix("context_").and_then(|f| occurrences.get(f)).copied().unwrap_or(*count);
        eprintln!("typed_cohort_family family={family} occurrences={observations} retained_records={count} bytes={bytes} largest={largest} conflicts=0");
    }
    let control = family_bytes.get("control").ok_or_else(invalid)?.1;
    eprintln!("typed_cohort_totals records={} encoded_bytes={total} control_bytes={control} sqlite_headroom={} pg_headroom={} sqlite_fits={} pg_fits={} reconstructed_presentation_bytes={reconstructed_bytes} original_model_reconstructed=true exact_cells=true presentations=17 conflicts=0 allocator_peak_measured=false pg_native_capture_qualified=false",decoded.len(),2_097_152i64-total as i64,1_048_576i64-total as i64,total<=2_097_152,total<=1_048_576);
    Ok(())
}
