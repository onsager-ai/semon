//! Test-only representation experiment. No persistence or reader consumes it.
use super::PreparedSources;
use serde::{Serialize, Serializer, ser::{SerializeMap, SerializeSeq, SerializeTuple}};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, BTreeSet}, io::{self, Write}};

const CODEC: &str = "typed-witness-cost-v1";
const SCRATCH: usize = 4096;

#[derive(Clone)]
struct Cell {
    key: String,
    metadata: String,
    recipe: Option<String>,
}

#[derive(Clone, Serialize)]
struct Trace {
    session: Value,
    turns: Value,
    edges: Value,
    bookmarks: Value,
}

/// Families are fixed by the existing eleven-element projection contract.
/// Only two same-owner cost fields are omitted; there are no field references.
#[derive(Clone)]
struct Witness {
    cells: Vec<Cell>,
    slots: Vec<Value>,
    traces: Vec<(String, Option<Trace>)>,
    pids: Value,
    lineage: Value,
    parents: Value,
    withdrawals: Value,
    endpoints: BTreeMap<String, Value>,
    relations: BTreeMap<String, Value>,
    messages: Value,
    retired: Value,
    lifecycle: Value,
    observations: Value,
}

fn invalid() -> io::Error {
    io::Error::other("typed witness ownership or copy mismatch")
}

impl Witness {
    fn capture(prepared: &PreparedSources) -> io::Result<Self> {
        let cells = prepared.selected_publications()?.into_iter().map(|p| Cell {
            key: p.row.key,
            metadata: p.metadata,
            recipe: p.query_recipe,
        }).collect::<Vec<_>>();
        let projection = prepared.staged_projection_witness()?;
        let p = projection.as_array().filter(|p| p.len() == 11).ok_or_else(invalid)?;
        let mut traces = Vec::new();
        for value in p[1].as_array().ok_or_else(invalid)? {
            let pair = value.as_array().filter(|p| p.len() == 2).ok_or_else(invalid)?;
            let key = pair[0].as_str().ok_or_else(invalid)?.to_owned();
            let trace = if pair[1].is_null() {
                None
            } else {
                let values = pair[1].as_array().filter(|v| v.len() == 4).ok_or_else(invalid)?;
                let cell = cells.iter().find(|c| c.key == key).ok_or_else(invalid)?;
                let metadata: Value = serde_json::from_str(&cell.metadata)?;
                let cost = metadata.get("cost").ok_or_else(invalid)?;
                let mut session = values[0].clone();
                // The trace is the projection attached to this exact producer
                // row. Both copies must equal its metadata cost before sharing.
                for copy in [session.get("cost"), session.get("details").and_then(|d| d.get("cost"))] {
                    if copy.is_none() || serde_json::to_vec(copy.unwrap())? != serde_json::to_vec(cost)? {
                        return Err(invalid());
                    }
                }
                session.as_object_mut().ok_or_else(invalid)?.remove("cost");
                session.get_mut("details").and_then(Value::as_object_mut).ok_or_else(invalid)?.remove("cost");
                Some(Trace { session, turns: values[1].clone(), edges: values[2].clone(), bookmarks: values[3].clone() })
            };
            traces.push((key, trace));
        }
        let result = Self {
            cells, slots: p[0].as_array().ok_or_else(invalid)?.clone(), traces,
            pids: p[2].clone(), lineage: p[3].clone(), parents: p[4].clone(), withdrawals: p[5].clone(),
            endpoints: serde_json::from_value(p[6].clone())?, relations: serde_json::from_value(p[7].clone())?,
            messages: p[8].clone(), retired: p[9].clone(), lifecycle: p[10].clone(),
            observations: serde_json::to_value(prepared.presentation_observations())?,
        };
        result.validate()?;
        Ok(result)
    }

    fn validate(&self) -> io::Result<()> {
        let mut keys = BTreeSet::new();
        for cell in &self.cells {
            if cell.metadata.len().saturating_add(cell.recipe.as_ref().map_or(0, String::len)) > 1048576 {
                return Err(invalid());
            }
            let metadata: Value = serde_json::from_str(&cell.metadata)?;
            if metadata["key"].as_str() != Some(cell.key.as_str()) || !keys.insert(cell.key.as_str()) {
                return Err(invalid());
            }
            crate::storage::proof_codec::validate_query_recipe_cell(&cell.metadata, cell.recipe.as_deref()).map_err(io::Error::other)?;
        }
        let mut trace_keys = BTreeSet::new();
        for (key, trace) in &self.traces {
            if !keys.contains(key.as_str()) || !trace_keys.insert(key.as_str()) {
                return Err(invalid());
            }
            if let Some(trace) = trace {
                let session = trace.session.as_object().ok_or_else(invalid)?;
                let details = session.get("details").and_then(Value::as_object).ok_or_else(invalid)?;
                if session.contains_key("cost") || details.contains_key("cost") {
                    return Err(invalid());
                }
            }
        }
        if keys != trace_keys { return Err(invalid()); }
        Ok(())
    }

    fn verify(&self, prepared: &PreparedSources) -> io::Result<()> {
        self.validate()?;
        let exact = prepared.selected_publications()?;
        if self.cells.len() != exact.len() || self.cells.iter().zip(exact).any(|(a, b)| a.key != b.row.key || a.metadata != b.metadata || a.recipe != b.query_recipe) {
            return Err(invalid());
        }
        let original = prepared.staged_model_witness()?;
        let mut compare = Compare { expected: original.as_bytes(), at: 0 };
        serde_json::to_writer(&mut compare, &Model { witness: self, reconstruct: true })?;
        if compare.at != original.len() { return Err(invalid()); }
        Ok(())
    }
}

/// Insert the two fixed cost facts in canonical map order without cloning a
/// session or assembling the reconstructed projection in another large Value.
struct Session<'a> { fields: &'a Map<String, Value>, cost: &'a Value, details: bool }
impl Serialize for Session<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.fields.len() + 1))?;
        let mut inserted = false;
        for (key, value) in self.fields {
            if !inserted && key.as_str() > "cost" {
                map.serialize_entry("cost", self.cost)?;
                inserted = true;
            }
            if self.details && key == "details" {
                let fields = value.as_object().ok_or_else(|| serde::ser::Error::custom("missing trace details"))?;
                map.serialize_entry(key, &Session { fields, cost: self.cost, details: false })?;
            } else { map.serialize_entry(key, value)?; }
        }
        if !inserted { map.serialize_entry("cost", self.cost)?; }
        map.end()
    }
}

struct TraceBody<'a> { trace: &'a Trace, cost: &'a Value }
impl Serialize for TraceBody<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let fields = self.trace.session.as_object().ok_or_else(|| serde::ser::Error::custom("missing trace session"))?;
        let mut tuple = serializer.serialize_tuple(4)?;
        tuple.serialize_element(&Session { fields, cost: self.cost, details: true })?;
        tuple.serialize_element(&self.trace.turns)?;
        tuple.serialize_element(&self.trace.edges)?;
        tuple.serialize_element(&self.trace.bookmarks)?;
        tuple.end()
    }
}

struct TraceRows<'a> { witness: &'a Witness, reconstruct: bool }
impl Serialize for TraceRows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.witness.traces.len()))?;
        for (key, trace) in &self.witness.traces {
            if self.reconstruct {
                // Parsing scratch is one bounded exact metadata cell. The
                // reconstructed output itself is streamed, never collected.
                let cell = self.witness.cells.iter().find(|c| c.key == *key).ok_or_else(|| serde::ser::Error::custom("missing owner cell"))?;
                let metadata: Value = serde_json::from_str(&cell.metadata).map_err(serde::ser::Error::custom)?;
                let cost = metadata.get("cost").ok_or_else(|| serde::ser::Error::custom("missing owner cost"))?;
                seq.serialize_element(&(key, trace.as_ref().map(|trace| TraceBody { trace, cost })))?;
            } else {
                // Preserve the old trace tuple shape; only the fixed cost
                // copies are absent under the explicit experiment codec.
                seq.serialize_element(&(key, trace.as_ref().map(|t| (&t.session, &t.turns, &t.edges, &t.bookmarks))))?;
            }
        }
        seq.end()
    }
}

struct Projection<'a> { witness: &'a Witness, reconstruct: bool }
impl Serialize for Projection<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let w = self.witness;
        let mut tuple = serializer.serialize_tuple(11)?;
        tuple.serialize_element(&w.slots)?;
        tuple.serialize_element(&TraceRows { witness: w, reconstruct: self.reconstruct })?;
        tuple.serialize_element(&w.pids)?;
        tuple.serialize_element(&w.lineage)?;
        tuple.serialize_element(&w.parents)?;
        tuple.serialize_element(&w.withdrawals)?;
        tuple.serialize_element(&w.endpoints)?;
        tuple.serialize_element(&w.relations)?;
        tuple.serialize_element(&w.messages)?;
        tuple.serialize_element(&w.retired)?;
        tuple.serialize_element(&w.lifecycle)?;
        tuple.end()
    }
}

struct Model<'a> { witness: &'a Witness, reconstruct: bool }
impl Serialize for Model<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut tuple = serializer.serialize_tuple(if self.reconstruct { 2 } else { 3 })?;
        if !self.reconstruct { tuple.serialize_element(CODEC)?; }
        tuple.serialize_element(&Projection { witness: self.witness, reconstruct: self.reconstruct })?;
        tuple.serialize_element(&self.witness.observations)?;
        tuple.end()
    }
}

struct Compare<'a> { expected: &'a [u8], at: usize }
impl Write for Compare<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.expected.get(self.at..self.at.saturating_add(bytes.len())) != Some(bytes) { return Err(invalid()); }
        self.at += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

/// Fixed scratch, no accumulating output. max_input is the largest slice
/// handed to Write, not an allocation/whole-process high-water measurement.
struct Sink { scratch: [u8; SCRATCH], used: usize, max_input: usize, bytes: usize, hash: Sha256 }
impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.max_input = self.max_input.max(bytes.len());
        for part in bytes.chunks(SCRATCH) {
            self.scratch[..part.len()].copy_from_slice(part);
            self.used = self.used.max(part.len());
            self.hash.update(&self.scratch[..part.len()]);
        }
        self.bytes += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

pub(crate) fn measure(value: &impl Serialize) -> io::Result<usize> {
    let mut sink = Sink { scratch: [0; SCRATCH], used: 0, max_input: 0, bytes: 0, hash: Sha256::new() };
    let state = std::mem::size_of::<serde_json::Serializer<&mut Sink>>();
    serde_json::to_writer(&mut sink, value)?;
    eprintln!("typed_witness_stream bytes={} scratch_capacity={} scratch_used={} serializer_state={} largest_write_slice={} sha256={:x}", sink.bytes, SCRATCH, sink.used, state, sink.max_input, sink.hash.finalize());
    Ok(sink.bytes)
}

pub(crate) fn qualify(prepared: &PreparedSources) -> io::Result<()> {
    let witness = Witness::capture(prepared)?;
    witness.verify(prepared)?;
    eprintln!("typed_witness_case name=genuine_exact_reconstruction status=pass");
    let cells = witness.cells.iter().map(|c| (&c.key, &c.metadata, &c.recipe)).collect::<Vec<_>>();
    let coordinator = measure(&cells)?;
    let normalized = measure(&Model { witness: &witness, reconstruct: false })?;
    let original = measure(&Model { witness: &witness, reconstruct: true })?;
    let aggregate = coordinator + normalized;
    eprintln!("typed_witness_memory sink_scratch={} reconstruct_metadata_input_max={} reconstruction_output_buffer=0 full_source_model_retained=true allocator_peak_measured=false", SCRATCH, witness.cells.iter().map(|c| c.metadata.len()).max().unwrap_or(0));
    let mut typed_bytes = 0usize;
    let mut row_count = 0usize;
    let mut row = |kind: &str, key: &str, value: &dyn Fn() -> io::Result<usize>| -> io::Result<()> {
        let bytes = value()?;
        eprintln!("typed_witness_row kind={kind} key={key} bytes={bytes}");
        if bytes > 1048576 { return Err(io::Error::other("typed row exceeds unchanged request budget")); }
        typed_bytes += bytes;
        row_count += 1;
        Ok(())
    };
    for cell in &witness.cells { row("cell", &cell.key, &|| measure(&(CODEC, "cell", &cell.key, &cell.metadata, &cell.recipe)))?; }
    for (key, trace) in &witness.traces { row("trace", key, &|| measure(&(CODEC, "trace", key, trace)))?; }
    for slot in &witness.slots {
        let key = slot[0].as_str().ok_or_else(invalid)?;
        row("slots", key, &|| measure(&(CODEC, "slots", slot)))?;
    }
    for (key, value) in &witness.endpoints { row("endpoint", key, &|| measure(&(CODEC, "endpoint", key, value)))?; }
    for (key, value) in &witness.relations { row("relation", key, &|| measure(&(CODEC, "relation", key, value)))?; }
    row("facts", "", &|| measure(&(CODEC, "facts", (&witness.pids, &witness.lineage, &witness.parents, &witness.withdrawals, &witness.messages, &witness.retired, &witness.lifecycle))))?;
    row("observations", "", &|| measure(&(CODEC, "observations", &witness.observations)))?;
    eprintln!("typed_witness_totals owners={} coordinator={} original_witness={} normalized_witness={} sqlite_aggregate={} sqlite_headroom={} typed_rows={} typed_bytes={} typed_framing_delta={}", witness.cells.len(), coordinator, original, normalized, aggregate, 2097152_i64 - aggregate as i64, row_count, typed_bytes, typed_bytes as i64 - aggregate as i64);
    if aggregate > 2097152 { return Err(io::Error::other("normalized witness exceeds unchanged SQLite coordinator budget")); }

    // Each mutation starts from the genuine shared producer result. These
    // exercise owner/cell authentication and reconstructed semantic evidence.
    let cases = ["missing_owner", "duplicate_owner", "wrong_owner", "metadata_bytes", "recipe_bytes", "trace_cost_copy", "trace_semantics", "slot_evidence", "relationship_evidence"];
    for case in cases {
        let mut changed = witness.clone();
        match case {
            "missing_owner" => { changed.cells.pop(); }
            "duplicate_owner" => changed.cells.push(changed.cells[0].clone()),
            "wrong_owner" => changed.traces[0].0 = "unowned-trace".into(),
            "metadata_bytes" => changed.cells[0].metadata.push(' '),
            "recipe_bytes" => changed.cells.iter_mut().find(|c| c.recipe.is_some()).ok_or_else(invalid)?.recipe.as_mut().unwrap().push(' '),
            "trace_cost_copy" => changed.traces.iter_mut().find_map(|(_, t)| t.as_mut()).ok_or_else(invalid)?.session["cost"] = Value::Null,
            "trace_semantics" => changed.traces.iter_mut().find_map(|(_, t)| t.as_mut()).ok_or_else(invalid)?.session["state"] = Value::String("tampered".into()),
            "slot_evidence" => changed.slots[0][1][0]["offset"] = Value::from(u64::MAX),
            "relationship_evidence" => { changed.relations.pop_first(); }
            _ => unreachable!(),
        }
        if changed.verify(prepared).is_ok() { return Err(io::Error::other(format!("typed witness accepted {case}"))); }
        eprintln!("typed_witness_case name={case} status=refused");
    }
    // Capture must refuse mismatching copies before any normalization. This
    // real producer row's cost is changed while its Trace copies stay intact.
    let mut wrong_cost = prepared.clone();
    wrong_cost.rows[0].cost.by_day.clear();
    wrong_cost.rows[0].cost.usd = Some(999.0);
    if Witness::capture(&wrong_cost).is_ok() { return Err(invalid()); }
    eprintln!("typed_witness_case name=producer_cost_copy_mismatch status=refused");
    eprintln!("typed_witness_cases positive=1 negative=10");
    Ok(())
}
