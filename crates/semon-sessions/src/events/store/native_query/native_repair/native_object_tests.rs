// Included inside native_repair::tests to reuse the native retained-evidence
// fixture. All semantic expected values come from serde_json::from_slice.
fn object_fixture(bytes: &[u8]) -> Fixture {
    let fixture = Fixture::new(bytes);
    fixture
        .store
        .connection
        .execute_batch(TEXT_VIEW_SCHEMA)
        .unwrap();
    fixture
        .store
        .connection
        .execute_batch(COMPARE_SCHEMA)
        .unwrap();
    fixture
        .store
        .connection
        .execute_batch(OBJECT_SCHEMA)
        .unwrap();
    fixture
}
fn check_object_work(work: &RepairWork) {
    assert!(
        work.metadata_rows <= ROWS,
        "object rows {}",
        work.metadata_rows
    );
    assert!(work.metadata_bytes <= BYTES);
    assert!(work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
}
fn object_build(f: &mut Fixture, r: &Repair, n: &OriginalNode) -> QualifiedObject {
    for attempt in 0..2_000_000 {
        let step = f
            .store
            .original_object_step(
                &f.job,
                &f.source,
                r,
                n,
                &f.sources.reader(),
                &|_, _| Ok(true),
                &|| false,
            )
            .unwrap();
        check_object_work(&step.work);
        assert_ne!(step.outcome, Outcome::Conflict);
        if let Some(object) = step.object {
            return object;
        }
        if attempt % 37 == 0 {
            f.store = NativeStore::open(&f.options).unwrap();
        }
    }
    panic!("native object failed eventual bounded progress")
}
fn object_entries(f: &Fixture, r: &Repair, o: &QualifiedObject) -> Vec<ObjectEntry> {
    let mut cursor = ObjectPageCursor::seek(o, 0);
    let mut entries = Vec::new();
    for _ in 0..o.len() * 4 + 128 {
        let step = f
            .store
            .original_object_page_step(
                &f.job,
                &f.source,
                r,
                &cursor,
                &f.sources.reader(),
                &|_, _| Ok(true),
                &|| false,
            )
            .unwrap();
        check_object_work(&step.work);
        assert_eq!(step.outcome, Outcome::Written);
        cursor = serde_json::from_str(&serde_json::to_string(&step.cursor).unwrap()).unwrap();
        if let Some(entry) = step.entry {
            entries.push(entry);
        }
        if step.complete {
            return entries;
        }
    }
    panic!("native object page cursor did not finish")
}
fn object_decode(f: &Fixture, r: &Repair, bytes: &[u8], entry: &ObjectEntry) -> (String, Value) {
    let key = entry.physical_key.span;
    let decoded: String =
        serde_json::from_slice(&bytes[key.start as usize - 1..key.end as usize + 1]).unwrap();
    assert_eq!(entry.key.original(), r.original);
    let (start, end) = match entry.value.value.clone() {
        OriginalValue::String(s) => (s.start - 1, s.end + 1),
        OriginalValue::Number(s) | OriginalValue::Literal { span: s, .. } => (s.start, s.end),
        OriginalValue::Object | OriginalValue::Array => {
            let end:i64=f.store.connection.query_row("SELECT end_offset FROM native_original_nodes WHERE original=?1 AND record_offset=?2 AND address=?3",params![r.original.as_slice(),r.cursor.record_at as i64,entry.value.address as i64],|v|v.get(0)).unwrap();
            (entry.value.address, end as u64 + 1)
        }
    };
    (
        decoded,
        serde_json::from_slice(&bytes[start as usize..end as usize]).unwrap(),
    )
}
#[test]
fn object_iterator_matches_native_last_duplicate_and_decoded_lexical_order() {
    let bytes="{\"z\":0,\"a\":1,\"\\u0061\":2,\"\":3,\"\\u0000\":4,\"é\":5,\"😀\":6,\"nested\":{\"z\":0,\"a\":1,\"a\":2},\"arr\":[{\"x\":1}],\"s\":\"\"}\n".as_bytes();
    let oracle: Value = serde_json::from_slice(bytes).unwrap();
    let mut f = object_fixture(bytes);
    let r = f.qualified_record();
    let root = f
        .store
        .original_root(&f.job, &f.source, &r)
        .unwrap()
        .unwrap();
    let object = object_build(&mut f, &r, &root);
    let actual: Vec<_> = object_entries(&f, &r, &object)
        .iter()
        .map(|e| object_decode(&f, &r, bytes, e))
        .collect();
    let expected: Vec<_> = oracle
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    assert_eq!(actual, expected);
    let nested = f.member(&r, "nested");
    let child = object_build(&mut f, &r, &nested);
    let actual: Vec<_> = object_entries(&f, &r, &child)
        .iter()
        .map(|e| object_decode(&f, &r, bytes, e))
        .collect();
    let expected: Vec<_> = oracle["nested"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    assert_eq!(actual, expected);
    // Every object in this record reuses the exact completed record map.
    let before: i64 = f
        .store
        .connection
        .query_row(
            "SELECT revision FROM native_original_record_key_maps WHERE original=?1",
            [r.original.as_slice()],
            |v| v.get(0),
        )
        .unwrap();
    let again = object_build(&mut f, &r, &root);
    assert_eq!(again.len(), object.len());
    assert_eq!(
        object_entries(&f, &r, &object).len(),
        oracle.as_object().unwrap().len()
    );
    let after: i64 = f
        .store
        .connection
        .query_row(
            "SELECT revision FROM native_original_record_key_maps WHERE original=?1",
            [r.original.as_slice()],
            |v| v.get(0),
        )
        .unwrap();
    assert_eq!(before, after);
}
#[test]
fn object_iterator_giant_common_prefix_huge_value_cancellation_and_reopen() {
    let prefix = "p".repeat(262_144);
    let huge = "v".repeat(200_000);
    let bytes =
        format!("{{\"{prefix}z\":0,\"{prefix}a\":1,\"huge\":\"{huge}\",\"{prefix}a\":2}}\n")
            .into_bytes();
    let oracle: Value = serde_json::from_slice(&bytes).unwrap();
    let mut f = object_fixture(&bytes);
    let r = f.qualified_record();
    let root = f
        .store
        .original_root(&f.job, &f.source, &r)
        .unwrap()
        .unwrap();
    for _ in 0..9 {
        let s = f
            .store
            .original_object_step(
                &f.job,
                &f.source,
                &r,
                &root,
                &f.sources.reader(),
                &|_, _| Ok(true),
                &|| false,
            )
            .unwrap();
        assert!(s.object.is_none());
        check_object_work(&s.work);
    }
    let before: i64 = f
        .store
        .connection
        .query_row(
            "SELECT coalesce(sum(revision),0) FROM native_original_objects",
            [],
            |v| v.get(0),
        )
        .unwrap();
    let cancelled = f
        .store
        .original_object_step(
            &f.job,
            &f.source,
            &r,
            &root,
            &f.sources.reader(),
            &|_, _| Ok(true),
            &|| true,
        )
        .unwrap();
    assert_eq!(cancelled.outcome, Outcome::Deferred);
    assert!(cancelled.object.is_none());
    assert_eq!(
        before,
        f.store
            .connection
            .query_row(
                "SELECT coalesce(sum(revision),0) FROM native_original_objects",
                [],
                |v| v.get::<_, i64>(0)
            )
            .unwrap()
    );
    f.store = NativeStore::open(&f.options).unwrap();
    let object = object_build(&mut f, &r, &root);
    let actual: Vec<_> = object_entries(&f, &r, &object)
        .iter()
        .map(|e| object_decode(&f, &r, &bytes, e))
        .collect();
    assert_eq!(
        actual,
        oracle
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>()
    );
    let cursor = ObjectPageCursor::seek(&object, 1);
    let step = f
        .store
        .original_object_page_step(
            &f.job,
            &f.source,
            &r,
            &cursor,
            &f.sources.reader(),
            &|_, _| Ok(true),
            &|| true,
        )
        .unwrap();
    assert!(step.entry.is_none());
    assert_eq!(step.outcome, Outcome::Deferred);
}
#[test]
fn object_iterator_rejects_nonobjects_malformed_and_descriptor_mutations() {
    for bytes in [b"{}\n".as_slice(), b"{\"a\":[]}\n".as_slice()] {
        let mut f = object_fixture(bytes);
        let r = f.qualified_record();
        let root = f
            .store
            .original_root(&f.job, &f.source, &r)
            .unwrap()
            .unwrap();
        let o = object_build(&mut f, &r, &root);
        if o.len() == 0 {
            assert!(object_entries(&f, &r, &o).is_empty());
        } else {
            let array = f.member(&r, "a");
            assert!(
                f.store
                    .original_object_step(
                        &f.job,
                        &f.source,
                        &r,
                        &array,
                        &f.sources.reader(),
                        &|_, _| Ok(true),
                        &|| false
                    )
                    .is_err()
            );
        }
    }
    for bytes in [b"{\"a\":]\n".as_slice(), b"[]\n".as_slice()] {
        let mut f = object_fixture(bytes);
        let r = f.qualified_record();
        assert!(f.store.original_root(&f.job, &f.source, &r).is_err());
    }
    let bytes = b"{\"a\":1,\"b\":2}\n";
    let mut f = object_fixture(bytes);
    let r = f.qualified_record();
    let root = f
        .store
        .original_root(&f.job, &f.source, &r)
        .unwrap()
        .unwrap();
    let o = object_build(&mut f, &r, &root);
    let cursor = ObjectPageCursor::seek(&o, 0);
    f.store.connection.execute("UPDATE native_original_fields SET decoded_bytes=decoded_bytes+1 WHERE original=?1 AND ordinal=(SELECT min(ordinal) FROM native_original_fields WHERE original=?1)",[r.original.as_slice()]).unwrap();
    let result = f.store.original_object_page_step(
        &f.job,
        &f.source,
        &r,
        &cursor,
        &f.sources.reader(),
        &|_, _| Ok(true),
        &|| false,
    );
    assert!(result.is_err() || result.unwrap().outcome == Outcome::Conflict);
}
#[test]
fn object_iterator_fixed_one_member_seek_work_grows_only_with_tree_height() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for size in [0usize, 128, 1024, 8192] {
        let bytes = format!(
            "{{{}}}\n",
            (0..size)
                .rev()
                .map(|i| format!("\"key{i:05}\":{i}"))
                .collect::<Vec<_>>()
                .join(",")
        )
        .into_bytes();
        let mut f = object_fixture(&bytes);
        let r = f.qualified_record();
        let root = f
            .store
            .original_root(&f.job, &f.source, &r)
            .unwrap()
            .unwrap();
        let o = object_build(&mut f, &r, &root);
        assert_eq!(o.len(), size as u64);
        let reference = reference(&f.sources.reader(), &f.source).unwrap();
        let provider = InterleavingReader::new(reference, &bytes, None, |_| {});
        // FileIndex loader work is measured after all background repair; no page
        // can hide whole selected Index loads in the provider/proof totals.
        let reads = super::super::super::super::FILE_READS.with(Cell::get);
        for rank in [0, size / 2, size.saturating_sub(1)] {
            let vm = Arc::new(AtomicUsize::new(0));
            let observed = vm.clone();
            f.store.connection.progress_handler(
                1,
                Some(move || {
                    observed.fetch_add(1, Ordering::Relaxed);
                    false
                }),
            );
            let calls = provider.calls.load(Ordering::SeqCst);
            let provider_bytes = provider.bytes_read.load(Ordering::SeqCst);
            let mut cursor = ObjectPageCursor::seek(&o, rank as u64);
            let mut rows = 0;
            let mut metadata = 0;
            let mut fragments = 0;
            let mut steps = 0;
            loop {
                let records = f.sources.reader_with(Some((
                    &provider,
                    crate::SessionSourceReadMode::ProviderOnly,
                )));
                let s = f
                    .store
                    .original_object_page_step(
                        &f.job,
                        &f.source,
                        &r,
                        &cursor,
                        &records,
                        &|_, _| Ok(true),
                        &|| false,
                    )
                    .unwrap();
                check_object_work(&s.work);
                assert_eq!(s.outcome, Outcome::Written);
                rows += s.work.metadata_rows;
                metadata += s.work.metadata_bytes;
                fragments += s.work.native_bytes;
                steps += 1;
                assert!(serde_json::to_vec(&s.cursor).unwrap().len() <= STATE_BYTES);
                cursor = s.cursor;
                if let Some(entry) = s.entry {
                    assert_eq!(
                        object_decode(&f, &r, &bytes, &entry),
                        (format!("key{rank:05}"), Value::from(rank))
                    );
                    break;
                }
                if s.complete {
                    assert_eq!(size, 0);
                    break;
                }
                assert!(steps < HEIGHT_FOR_TEST);
            }
            f.store.connection.progress_handler(0, None::<fn() -> bool>);
            let calls = provider.calls.load(Ordering::SeqCst) - calls;
            let bytes_read = provider.bytes_read.load(Ordering::SeqCst) - provider_bytes;
            let height_bound = if size == 0 {
                1
            } else {
                2 * ((size + 1).ilog2() as usize + 1)
            };
            assert!(steps <= height_bound);
            assert!(rows <= 64 * height_bound);
            assert!(metadata <= 64 * 1024 * height_bound);
            assert!(fragments <= height_bound);
            assert!(calls <= 16 * height_bound);
            assert!(bytes_read <= 16 * crate::source_chunks::SIZE * height_bound);
            assert!(vm.load(Ordering::Relaxed) < 100_000 * height_bound);
            eprintln!(
                "object size={size} rank={rank} steps={steps} sql_vm={} rows={rows} metadata={metadata} logical_native={fragments} actual_provider_calls={calls} actual_provider_bytes={bytes_read}",
                vm.load(Ordering::Relaxed)
            );
        }
        assert_eq!(
            reads,
            super::super::super::super::FILE_READS.with(Cell::get)
        );
    }
}
const HEIGHT_FOR_TEST: usize = 96;
#[test]
fn object_iterator_provider_only_retained_archive_and_post_io_fences() {
    use std::sync::atomic::Ordering;
    let bytes = b"{\"b\":2,\"a\":1}\n";
    let mut f = object_fixture(bytes);
    let r = f.qualified_record();
    let root = f
        .store
        .original_root(&f.job, &f.source, &r)
        .unwrap()
        .unwrap();
    let source = reference(&f.sources.reader(), &f.source).unwrap();
    fs::remove_file(&f.source.source.path).unwrap();
    let provider = InterleavingReader::new(source.clone(), bytes, None, |_| {});
    let mut object = None;
    for _ in 0..200 {
        let records = f.sources.reader_with(Some((
            &provider,
            crate::SessionSourceReadMode::ProviderOnly,
        )));
        let s = f
            .store
            .original_object_step(
                &f.job,
                &f.source,
                &r,
                &root,
                &records,
                &|_, _| Ok(true),
                &|| false,
            )
            .unwrap();
        check_object_work(&s.work);
        assert_ne!(s.outcome, Outcome::Conflict);
        if s.object.is_some() {
            object = s.object;
            break;
        }
    }
    let object = object.expect("retained provider-only object must complete");
    assert!(provider.calls.load(Ordering::SeqCst) > 0);
    let cursor = ObjectPageCursor::seek(&object, 0);
    let changing = InterleavingReader::new(source.clone(), bytes, Some(0), |_| {});
    let records = f.sources.reader_with(Some((
        &changing,
        crate::SessionSourceReadMode::ProviderOnly,
    )));
    assert!(
        f.store
            .original_object_page_step(
                &f.job,
                &f.source,
                &r,
                &cursor,
                &records,
                &|_, _| Ok(true),
                &|| false
            )
            .is_err()
    );
    let denied = crate::query_source::Sources::capture(&f.options, std::iter::empty()).unwrap();
    assert!(
        f.store
            .original_object_page_step(
                &f.job,
                &f.source,
                &r,
                &cursor,
                &denied.reader(),
                &|_, _| Ok(true),
                &|| false
            )
            .is_err()
    );
    // Mutation in the final external closure runs after read-snapshot release.
    let calls = Cell::new(0);
    let closure = |db: &Connection, _| {
        let at = calls.get() + 1;
        calls.set(at);
        if at == 3 {
            db.execute(
                "UPDATE session_catalog SET last_ms=last_ms+1 WHERE session_key='chosen'",
                [],
            )?;
        }
        Ok(true)
    };
    let records = f.sources.reader_with(Some((
        &provider,
        crate::SessionSourceReadMode::ProviderOnly,
    )));
    let s = f
        .store
        .original_object_page_step(&f.job, &f.source, &r, &cursor, &records, &closure, &|| {
            false
        })
        .unwrap();
    assert_eq!(s.outcome, Outcome::Conflict);
    assert!(s.entry.is_none());
}
#[test]
fn object_iterator_avl_height_bound_covers_u64_and_corrupt_tree_refuses() {
    let (mut prev, mut current) = (0u128, 1u128);
    for _ in 2..=96 {
        let next = 1 + prev + current;
        prev = current;
        current = next;
    }
    assert!(current > u128::from(u64::MAX));
    let bytes = b"{\"c\":3,\"a\":1,\"b\":2}\n";
    let mut f = object_fixture(bytes);
    let r = f.qualified_record();
    let root = f
        .store
        .original_root(&f.job, &f.source, &r)
        .unwrap()
        .unwrap();
    let o = object_build(&mut f, &r, &root);
    f.store
        .connection
        .execute(
            "UPDATE native_original_object_nodes SET metadata=json_set(metadata,'$.height',95)",
            [],
        )
        .unwrap();
    assert!(
        f.store
            .original_object_page_step(
                &f.job,
                &f.source,
                &r,
                &ObjectPageCursor::seek(&o, 0),
                &f.sources.reader(),
                &|_, _| Ok(true),
                &|| false
            )
            .is_err()
    );
}
#[test]
fn object_iterator_cancellation_rolls_back_map_writes_and_identity_alias_is_exact() {
    let bytes = b"{\"a\":0,\"\\u0061\":1}\n";
    let mut f = object_fixture(bytes);
    let r = f.qualified_record();
    let root = f
        .store
        .original_root(&f.job, &f.source, &r)
        .unwrap()
        .unwrap();
    let calls = Cell::new(0);
    let cancel = || {
        let at = calls.get() + 1;
        calls.set(at);
        at == 3
    };
    let s = f
        .store
        .original_object_step(
            &f.job,
            &f.source,
            &r,
            &root,
            &f.sources.reader(),
            &|_, _| Ok(true),
            &cancel,
        )
        .unwrap();
    assert_eq!(s.outcome, Outcome::Deferred);
    let rows:i64=f.store.connection.query_row("SELECT (SELECT count(*) FROM native_original_record_keys)+(SELECT count(*) FROM native_original_record_key_maps)",[],|v|v.get(0)).unwrap();
    assert_eq!(rows, 0);
    let object = object_build(&mut f, &r, &root);
    let entries = object_entries(&f, &r, &object);
    assert_eq!(entries.len(), 1);
    assert_eq!(
        object_decode(&f, &r, bytes, &entries[0]),
        ("a".into(), Value::from(1))
    );
    let representative = f
        .store
        .original_text_representative(&f.job, &f.source, &r, &entries[0].key)
        .unwrap();
    let native_values::Value::Whole { field, .. } = representative.value else {
        panic!("whole representative expected")
    };
    assert_ne!(
        field.ordinal, entries[0].physical_key.ordinal,
        "last physical duplicate must resolve the earlier canonical representative"
    );
    f.store.connection.execute("UPDATE native_original_identities SET cursor=json_set(cursor,'$.class',999999) WHERE original=?1 AND ordinal=?2",params![r.original.as_slice(),entries[0].physical_key.ordinal as i64]).unwrap();
    assert!(
        f.store
            .original_object_page_step(
                &f.job,
                &f.source,
                &r,
                &ObjectPageCursor::seek(&object, 0),
                &f.sources.reader(),
                &|_, _| Ok(true),
                &|| false
            )
            .is_err()
    );
}
#[test]
fn object_iterator_all_publication_custody_and_relationship_fences_after_provider_io() {
    for mutation in [
        "owner",
        "header",
        "archive",
        "claim",
        "mutation",
        "job",
        "repair",
        "relationship",
    ] {
        let bytes = b"{\"a\":1}\n";
        let mut f = object_fixture(bytes);
        let r = f.qualified_record();
        let root = f
            .store
            .original_root(&f.job, &f.source, &r)
            .unwrap()
            .unwrap();
        let object = object_build(&mut f, &r, &root);
        let db_path = super::super::super::super::EventCache::path(&f.options.cache);
        let path = f.source.source.path.to_string_lossy().to_string();
        let original = r.original;
        let job = f.job.id;
        f.store.connection.execute_batch("CREATE TABLE object_related_owner(generation INTEGER);INSERT INTO object_related_owner VALUES(1);").unwrap();
        let closure = |db: &Connection, _| {
            db.query_row("SELECT generation=1 FROM object_related_owner", [], |v| {
                v.get(0)
            })
        };
        let provider = InterleavingReader::new(
            reference(&f.sources.reader(), &f.source).unwrap(),
            bytes,
            None,
            move |call| {
                if call != 0 {
                    return;
                }
                let db = Connection::open(&db_path).unwrap();
                match mutation {
                    "owner" => {
                        db.execute("UPDATE session_catalog SET last_ms=last_ms+1 WHERE session_key='chosen'",[]).unwrap();
                    }
                    "header" => {
                        db.execute("INSERT OR REPLACE INTO session_trace_headers VALUES('chosen','changed','changed','{}')",[]).unwrap();
                    }
                    "archive" => {
                        db.execute(
                            "INSERT OR REPLACE INTO meta VALUES(?1,'changed')",
                            [format!("archive_event_index:{path}")],
                        )
                        .unwrap();
                    }
                    "claim" => {
                        db.execute("INSERT INTO session_catalog_invalidations(source_path,revision,reason,observed_at) VALUES(?1,1,'changed',0) ON CONFLICT(source_path) DO UPDATE SET revision=revision+1",[&path]).unwrap();
                    }
                    "mutation" => {
                        db.execute(
                            "INSERT OR REPLACE INTO session_source_mutations VALUES(?1,'changed')",
                            [&path],
                        )
                        .unwrap();
                    }
                    "job" => {
                        db.execute(
                            "UPDATE native_query_jobs SET revision=revision+1 WHERE job=?1",
                            [job],
                        )
                        .unwrap();
                    }
                    "repair" => {
                        db.execute("UPDATE native_original_repairs SET revision=revision+1 WHERE original=?1",[original.as_slice()]).unwrap();
                    }
                    "relationship" => {
                        db.execute("UPDATE object_related_owner SET generation=2", [])
                            .unwrap();
                    }
                    _ => unreachable!(),
                }
            },
        );
        let records = f.sources.reader_with(Some((
            &provider,
            crate::SessionSourceReadMode::ProviderOnly,
        )));
        let result = f.store.original_object_page_step(
            &f.job,
            &f.source,
            &r,
            &ObjectPageCursor::seek(&object, 0),
            &records,
            &closure,
            &|| false,
        );
        assert!(
            result.is_err() || result.unwrap().outcome == Outcome::Conflict,
            "mutation={mutation}"
        );
    }
}
#[test]
fn object_iterator_called_retirement_precedes_classes_and_never_decodes_old_state() {
    let bytes = b"{\"a\":1}\n";
    let mut f = object_fixture(bytes);
    let r = f.qualified_record();
    let root = f
        .store
        .original_root(&f.job, &f.source, &r)
        .unwrap()
        .unwrap();
    let _object = object_build(&mut f, &r, &root);
    let old = previous_namespace(&r.source).unwrap();
    // Intentionally invalid, oversized obsolete state must be retired by stable
    // keys. Cleanup must not recursively materialize or deserialize its bodies.
    for sql in [
        "INSERT INTO native_original_object_nodes SELECT ?1,record_offset,object_address,class,printf('%.*c',70000,'x') FROM native_original_object_nodes WHERE original=?2",
        "INSERT INTO native_original_objects SELECT ?1,record_offset,object_address,revision,'invalid' FROM native_original_objects WHERE original=?2",
        "INSERT INTO native_original_record_keys SELECT ?1,record_offset,parent_offset,ordinal,key_start,value_address FROM native_original_record_keys WHERE original=?2",
        "INSERT INTO native_original_record_key_maps SELECT ?1,record_offset,revision,'invalid' FROM native_original_record_key_maps WHERE original=?2",
        "INSERT INTO native_original_object_epochs SELECT ?1,record_offset,revision FROM native_original_object_epochs WHERE original=?2",
    ] {
        f.store
            .connection
            .execute(sql, params![old.as_slice(), r.original.as_slice()])
            .unwrap();
    }
    f.store
        .connection
        .execute(
            "INSERT OR IGNORE INTO native_original_object_epochs VALUES(?1,?2,0)",
            params![old.as_slice(), r.cursor.record_at as i64],
        )
        .unwrap();
    native_retirement::enqueue(&f.store.connection, r.original, &r.source).unwrap();
    let cancelled = f
        .store
        .original_cleanup_step(&f.job, &f.source, &r, &f.sources.reader(), &|| true)
        .unwrap();
    assert_eq!(cancelled.outcome, Outcome::Deferred);
    for _ in 0..128 {
        if !native_retirement::pending(&f.store.connection, r.original).unwrap() {
            break;
        }
        let step = f
            .store
            .original_cleanup_step(&f.job, &f.source, &r, &f.sources.reader(), &|| false)
            .unwrap();
        check_object_work(&step.work);
        f.store = NativeStore::open(&f.options).unwrap();
    }
    assert!(!native_retirement::pending(&f.store.connection, r.original).unwrap());
    for table in [
        "native_original_object_nodes",
        "native_original_objects",
        "native_original_record_keys",
        "native_original_record_key_maps",
        "native_original_object_epochs",
    ] {
        let count: i64 = f
            .store
            .connection
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE original=?1"),
                [old.as_slice()],
                |v| v.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
