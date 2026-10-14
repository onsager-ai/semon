//! Rebuildable native physical-star profiles. Native producers stream exact
//! raw atom parts; foreground selection reads only completed scalar/pair facts.
//! No Index, native link set, authority or provider is owned by this module.
use super::*;
use crate::model::identity_stage::physical::{
    NativeScanOrder, PhysicalRole, PhysicalSource, ProfileRoot,
};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;

pub(crate) const PART: usize = 64 * 1024;
const KEY_BYTES: usize = 4096;
pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_physical_link_revisions (
 path TEXT PRIMARY KEY, revision INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_physical_bootstrap (
 path TEXT PRIMARY KEY, wide_links INTEGER NOT NULL CHECK(wide_links>=0)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_physical_profiles (
 root BLOB PRIMARY KEY CHECK(length(root)=32), path TEXT NOT NULL, revision INTEGER NOT NULL,
 descriptor TEXT NOT NULL, phase TEXT NOT NULL, original BLOB,
 next_atom INTEGER NOT NULL DEFAULT 0, atom INTEGER,
 minimum INTEGER, compare_atom INTEGER, compare_part INTEGER NOT NULL DEFAULT 0,
 peer_after INTEGER NOT NULL DEFAULT 0, continued INTEGER NOT NULL DEFAULT 0
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_physical_profile_path ON native_physical_profiles(path,revision);
CREATE TABLE IF NOT EXISTS native_physical_atoms (
 atom INTEGER PRIMARY KEY, root BLOB NOT NULL CHECK(length(root)=32), ordinal INTEGER NOT NULL,
 kind TEXT NOT NULL, length INTEGER NOT NULL DEFAULT 0,
 digest BLOB NOT NULL, complete INTEGER NOT NULL DEFAULT 0,
 UNIQUE(root,ordinal)
) STRICT;
CREATE INDEX IF NOT EXISTS native_physical_atom_bucket ON native_physical_atoms(digest,length,atom) WHERE complete=1;
CREATE TABLE IF NOT EXISTS native_physical_atom_parts (
 atom INTEGER NOT NULL, part INTEGER NOT NULL, bytes BLOB NOT NULL,
 PRIMARY KEY(atom,part), CHECK(length(bytes)<=65536)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_physical_part_hashes (
 atom INTEGER NOT NULL, part INTEGER NOT NULL, digest BLOB NOT NULL CHECK(length(digest)=32),
 PRIMARY KEY(atom,part)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_physical_counts (
 root BLOB PRIMARY KEY, atoms INTEGER NOT NULL DEFAULT 0, complete INTEGER NOT NULL DEFAULT 0,
 expected_parts INTEGER NOT NULL DEFAULT 0, parts INTEGER NOT NULL DEFAULT 0, hashes INTEGER NOT NULL DEFAULT 0
) STRICT, WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS native_physical_atom_insert AFTER INSERT ON native_physical_atoms BEGIN
 INSERT OR IGNORE INTO native_physical_counts(root) VALUES(NEW.root);
 UPDATE native_physical_counts SET atoms=atoms+1,complete=complete+NEW.complete,expected_parts=expected_parts+NEW.complete*(NEW.length/65536+(NEW.length%65536<>0)+(NEW.length=0)) WHERE root=NEW.root;
END;
CREATE TRIGGER IF NOT EXISTS native_physical_atom_update AFTER UPDATE ON native_physical_atoms BEGIN
 UPDATE native_physical_counts SET atoms=atoms-1,complete=complete-OLD.complete,expected_parts=expected_parts-OLD.complete*(OLD.length/65536+(OLD.length%65536<>0)+(OLD.length=0)) WHERE root=OLD.root;
 INSERT OR IGNORE INTO native_physical_counts(root) VALUES(NEW.root);
 UPDATE native_physical_counts SET atoms=atoms+1,complete=complete+NEW.complete,expected_parts=expected_parts+NEW.complete*(NEW.length/65536+(NEW.length%65536<>0)+(NEW.length=0)) WHERE root=NEW.root;
END;
CREATE TRIGGER IF NOT EXISTS native_physical_atom_delete BEFORE DELETE ON native_physical_atoms BEGIN
 UPDATE native_physical_counts SET atoms=atoms-1,complete=complete-OLD.complete,expected_parts=expected_parts-OLD.complete*(OLD.length/65536+(OLD.length%65536<>0)+(OLD.length=0)) WHERE root=OLD.root;
END;
CREATE TRIGGER IF NOT EXISTS native_physical_part_insert AFTER INSERT ON native_physical_atom_parts BEGIN
 UPDATE native_physical_counts SET parts=parts+1 WHERE root=(SELECT root FROM native_physical_atoms WHERE atom=NEW.atom);
END;
CREATE TRIGGER IF NOT EXISTS native_physical_part_delete BEFORE DELETE ON native_physical_atom_parts BEGIN
 UPDATE native_physical_counts SET parts=parts-1 WHERE root=(SELECT root FROM native_physical_atoms WHERE atom=OLD.atom);
END;
CREATE TRIGGER IF NOT EXISTS native_physical_hash_insert AFTER INSERT ON native_physical_part_hashes BEGIN
 UPDATE native_physical_counts SET hashes=hashes+1 WHERE root=(SELECT root FROM native_physical_atoms WHERE atom=NEW.atom);
END;
CREATE TRIGGER IF NOT EXISTS native_physical_hash_delete BEFORE DELETE ON native_physical_part_hashes BEGIN
 UPDATE native_physical_counts SET hashes=hashes-1 WHERE root=(SELECT root FROM native_physical_atoms WHERE atom=OLD.atom);
END;
CREATE TABLE IF NOT EXISTS native_physical_pairs (
 left_root BLOB NOT NULL, right_root BLOB NOT NULL,
 PRIMARY KEY(left_root,right_root), CHECK(left_root<right_root AND length(left_root)=32 AND length(right_root)=32)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_physical_reverse_pair ON native_physical_pairs(right_root,left_root);
CREATE TABLE IF NOT EXISTS native_physical_bindings (
 source BLOB PRIMARY KEY CHECK(length(source)=32), root BLOB NOT NULL CHECK(length(root)=32), first_ms INTEGER
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_physical_session_bucket ON native_physical_atoms(root,kind,digest,length,atom) WHERE complete=1;
CREATE TABLE IF NOT EXISTS native_physical_orders (
 left_root BLOB NOT NULL, right_root BLOB NOT NULL,
 part INTEGER NOT NULL DEFAULT 0, ordering INTEGER CHECK(ordering IN (-1,0,1)),
 PRIMARY KEY(left_root,right_root), CHECK(left_root<right_root)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_physical_current_cursors (
 root BLOB PRIMARY KEY, ordinal INTEGER NOT NULL, kind TEXT NOT NULL, native_id TEXT NOT NULL,
 CHECK(octet_length(kind)<=16 AND octet_length(native_id)<=4096)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_physical_truncations (
 root BLOB NOT NULL, context BLOB NOT NULL, next INTEGER NOT NULL CHECK(next>=0), answer INTEGER CHECK(answer IN (0,1)), candidate INTEGER NOT NULL DEFAULT 0 CHECK(candidate>=0),
 PRIMARY KEY(root,context)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_physical_fragments (
 root BLOB NOT NULL, ordinal INTEGER NOT NULL CHECK(ordinal>=0), offset INTEGER NOT NULL CHECK(offset>=0),
 kind TEXT NOT NULL CHECK(octet_length(kind)<=16), bytes BLOB NOT NULL CHECK(octet_length(bytes)<=65536),
 last INTEGER NOT NULL CHECK(last IN (0,1)), PRIMARY KEY(root,ordinal,offset)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_physical_carry (
 root BLOB PRIMARY KEY, ordinal INTEGER NOT NULL CHECK(ordinal>=0), kind TEXT NOT NULL CHECK(octet_length(kind)<=16),
 received INTEGER NOT NULL CHECK(received>=0), consumed INTEGER NOT NULL CHECK(consumed>=0),
 offset INTEGER NOT NULL CHECK(offset>=0), bytes BLOB NOT NULL CHECK(octet_length(bytes)<65536),
 last INTEGER NOT NULL CHECK(last IN (0,1))
) STRICT, WITHOUT ROWID;
";

/// Called in the native writer transaction before replacing old link rows.
/// Event-only appends execute one scalar lookup and do not touch profiles.
pub(in crate::events::store) fn capture_revision(
    tx: &Connection,
    path: &str,
    changes: Option<&super::super::super::DirtyRows>,
    index: Option<&FileIndex>,
) -> rusqlite::Result<()> {
    let mut changed = changes.is_none();
    let mut wide_delta = 0i64;
    if let (Some(changes), Some(index)) = (changes, index) {
        let file: Option<i64> = tx
            .query_row("SELECT file_id FROM files WHERE path=?1", [path], |r| {
                r.get(0)
            })
            .optional()?;
        for (kind, id) in &changes.links {
            let before=match file { Some(file)=>tx.query_row("SELECT EXISTS(SELECT 1 FROM native_source_links WHERE file_id=?1 AND kind=?2 AND native_id=?3)",params![file,kind,id],|r|r.get::<_,bool>(0))?,None=>false };
            let after = match kind.as_str() {
                "session" => index.links.session_ids.contains(id),
                "continued" => index.links.continued_in.contains(id),
                "bridge" => index.links.bridges.contains(id),
                _ => return Err(invalid("invalid physical link kind")),
            };
            changed |= before != after;
            if id.len() > KEY_BYTES {
                wide_delta += i64::from(after) - i64::from(before);
            }
        }
    }
    match (changes, index) {
        (None, Some(index)) => {
            let count = index
                .links
                .session_ids
                .iter()
                .chain(&index.links.continued_in)
                .chain(&index.links.bridges)
                .filter(|id| id.len() > KEY_BYTES)
                .count();
            tx.execute("INSERT INTO native_physical_bootstrap VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET wide_links=excluded.wide_links",params![path,sequence_sql(count)?])?;
        }
        (Some(_), Some(_)) => {
            tx.execute(
                "UPDATE native_physical_bootstrap SET wide_links=wide_links+?2 WHERE path=?1",
                params![path, wide_delta],
            )?;
        }
        (_, None) => {
            tx.execute(
                "DELETE FROM native_physical_bootstrap WHERE path=?1",
                [path],
            )?;
        }
    }
    if !changed {
        tx.execute(
            "INSERT OR IGNORE INTO native_physical_link_revisions VALUES(?1,1)",
            [path],
        )?;
    } else {
        tx.execute("INSERT INTO native_physical_link_revisions VALUES(?1,1) ON CONFLICT(path) DO UPDATE SET revision=revision+1", [path])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    };

    struct Fixture {
        root: PathBuf,
        store: NativeStore,
        witness: SourceWitness,
        index: FileIndex,
    }
    impl Fixture {
        fn admit_source(&mut self, witness: &SourceWitness) -> Job {
            let spec = JobSpec {
                view_key: "profile-native-owner".into(),
                version: 1,
                catalog_generation: None,
                runtime: crate::native_evidence::query_runtime_generation(&self.store.options)
                    .unwrap(),
            };
            let job = self.store.begin(&spec).unwrap();
            assert_eq!(
                self.store
                    .admit(&job, std::slice::from_ref(witness), &[], "profile")
                    .unwrap(),
                Outcome::Written
            );
            self.store.reload(&spec.view_key).unwrap().unwrap()
        }
        fn repair(
            &mut self,
            job: &Job,
            witness: &SourceWitness,
        ) -> super::super::native_repair::Repair {
            let sources = crate::query_source::Sources::capture(
                &self.store.options,
                std::iter::once((witness.source.clone(), None)),
            )
            .unwrap();
            let records = sources.reader();
            let mut repair = self
                .store
                .begin_original_repair(job, witness, &records, &|| false)
                .unwrap()
                .unwrap();
            for _ in 0..128 {
                let phase:String=self.store.connection.query_row("SELECT json_extract(cursor,'$.phase') FROM native_original_repairs WHERE original=?1",[repair.original.as_slice()],|r|r.get(0)).unwrap();
                if phase == "Complete" {
                    return repair;
                }
                if phase == "FoldRecord" {
                    self.store
                        .original_gap_step(job, witness, &repair, &records, &|| false)
                        .unwrap();
                } else {
                    self.store
                        .original_lex_step(job, witness, &repair, &records, &|| false)
                        .unwrap();
                }
                repair = self
                    .store
                    .reload_original_repair(job, witness, &records)
                    .unwrap()
                    .unwrap();
            }
            panic!("Original fixture repair did not complete")
        }
        fn add_source(&self, id: &str, session: &[&str], bridge: &[&str]) -> SourceWitness {
            let path = self
                .store
                .options
                .claude_home
                .join(format!("projects/p/{id}.jsonl"));
            fs::write(&path, b"fixture\n").unwrap();
            let ledger = Ledger {
                stat: Stat::of(&path).unwrap(),
                offset: 8,
                prefix: Sha256::digest(b"fixture\n").into(),
                tail: Sha256::digest(b"fixture\n").into(),
            };
            let mut index = FileIndex::default();
            index.links.session_ids = session.iter().map(|v| (*v).into()).collect();
            index.links.bridges = bridge.iter().map(|v| (*v).into()).collect();
            let tx = self.store.connection.unchecked_transaction().unwrap();
            write_index_changes(&tx, path.to_str().unwrap(), &ledger, &index, None, None).unwrap();
            tx.commit().unwrap();
            let mut source = ledger.source(&path, id);
            source.native_role = Some(serde_json::json!({"Top":{"slug":"p"}}));
            self.store.source_witness(&source).unwrap()
        }
        fn prepare_source(&self, witness: &SourceWitness) -> ProfileRoot {
            for _ in 0..100000 {
                let (root, progress, _) = self
                    .store
                    .physical_profile_current_step(witness, &|| false)
                    .unwrap();
                if progress == ProfileProgress::Ready {
                    return root;
                }
                assert_ne!(progress, ProfileProgress::NativeAtomsRequired);
            }
            panic!("physical source profile did not progress")
        }
        fn new(id: &str, session: &[&str], bridge: &[&str]) -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-physical-profile-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let options = crate::Options {
                claude_home: root.join("claude"),
                claude_json: root.join("claude.json"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                cache: root.join("cache"),
                ..crate::Options::default()
            };
            let path = options.claude_home.join(format!("projects/p/{id}.jsonl"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"fixture\n").unwrap();
            let ledger = Ledger {
                stat: Stat::of(&path).unwrap(),
                offset: 8,
                prefix: Sha256::digest(b"fixture\n").into(),
                tail: Sha256::digest(b"fixture\n").into(),
            };
            let store = NativeStore::open(&options).unwrap();
            // Parent schema registration is integrated independently. This
            // focused fixture always exercises the module's exact own schema.
            store.connection.execute_batch(SCHEMA).unwrap();
            let mut index = FileIndex::default();
            index.links.session_ids = session.iter().map(|id| (*id).into()).collect();
            index.links.bridges = bridge.iter().map(|id| (*id).into()).collect();
            let tx = store.connection.unchecked_transaction().unwrap();
            write_index_changes(&tx, path.to_str().unwrap(), &ledger, &index, None, None).unwrap();
            tx.commit().unwrap();
            let mut source = ledger.source(&path, id);
            source.native_role = Some(serde_json::json!({"Top":{"slug":"p"}}));
            let witness = store.source_witness(&source).unwrap();
            Self {
                root,
                store,
                witness,
                index,
            }
        }
        fn prepare(&self) -> ProfileRoot {
            for _ in 0..10000 {
                let (root, progress, _) = self
                    .store
                    .physical_profile_current_step(&self.witness, &|| false)
                    .unwrap();
                if progress == ProfileProgress::Ready {
                    return root;
                }
                assert_ne!(progress, ProfileProgress::NativeAtomsRequired);
            }
            panic!("physical profile made no progress")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn native_original_target_reuses_current_root_after_event_only_append() {
        for length in [PART, 1024 * 1024, 1024 * 1024 + 17] {
            let atom = "x".repeat(length);
            let mut fixture = Fixture::new("current", &[&atom], &[]);
            let witness = fixture.witness.clone();
            let mut current = None;
            for _ in 0..16 {
                let (root, p, work) = fixture
                    .store
                    .physical_profile_current_step(&witness, &|| false)
                    .unwrap();
                assert!(work.metadata_bytes <= PART);
                if p == ProfileProgress::NativeAtomsRequired {
                    current = Some(root);
                    break;
                }
            }
            let current = current.expect("wide native links must route before raw SQL bootstrap");
            assert!(!fixture.store.connection.query_row("SELECT EXISTS(SELECT 1 FROM native_physical_current_cursors WHERE root=?1)",[current.0.as_slice()],|r|r.get::<_,bool>(0)).unwrap());
            let job = fixture.admit_source(&witness);
            let repair = fixture.repair(&job, &witness);
            let sources = crate::query_source::Sources::capture(
                &fixture.store.options,
                std::iter::once((witness.source.clone(), None)),
            )
            .unwrap();
            let records = sources.reader();
            let operation = OriginalProfileOperation {
                job: &job,
                repair: &repair,
                records: &records,
            };
            let (root, _) = fixture
                .store
                .physical_profile_original_target(
                    operation,
                    &witness,
                    None,
                    PhysicalRole::Top,
                    NativeScanOrder::new(PhysicalRole::Top, &witness.source.path),
                )
                .unwrap();
            assert_eq!(root, current);
            fixture
                .store
                .physical_profile_original_part(
                    operation,
                    &witness,
                    root,
                    0,
                    0,
                    AtomKind::Own,
                    b"current",
                    true,
                )
                .unwrap();
            for (i, bytes) in atom.as_bytes().chunks(PART).enumerate() {
                let offset = (i * PART) as u64;
                let last = (i + 1) * PART >= atom.len();
                fixture
                    .store
                    .physical_profile_original_part(
                        operation,
                        &witness,
                        root,
                        1,
                        offset,
                        AtomKind::Session,
                        bytes,
                        last,
                    )
                    .unwrap();
                if i == 0 {
                    let reopened = NativeStore::open(&fixture.store.options).unwrap();
                    fixture.store = reopened;
                }
                // Simulates cancellation after part commit and before the
                // native extraction cursor CAS, then a reopened exact replay.
                fixture
                    .store
                    .physical_profile_original_part(
                        operation,
                        &witness,
                        root,
                        1,
                        offset,
                        AtomKind::Session,
                        bytes,
                        last,
                    )
                    .unwrap();
            }
            while fixture
                .store
                .physical_profile_original_advance(operation, &witness, root, &|| false)
                .unwrap()
                .0
                == ProfileProgress::Pending
            {}
            fixture
                .store
                .physical_profile_original_finish(operation, &witness, root, 2)
                .unwrap();
            assert_eq!(
                fixture
                    .store
                    .physical_profile_original_part(
                        operation,
                        &witness,
                        root,
                        1,
                        0,
                        AtomKind::Session,
                        &atom.as_bytes()[..PART],
                        length == PART
                    )
                    .unwrap(),
                ProfileProgress::Ready
            );
            assert!(
                fixture
                    .store
                    .physical_profile_original_part(
                        operation,
                        &witness,
                        root,
                        2,
                        0,
                        AtomKind::Session,
                        b"new",
                        true
                    )
                    .is_err()
            );
            let before_atoms: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT atoms FROM native_physical_counts WHERE root=?1",
                    [root.0.as_slice()],
                    |r| r.get(0),
                )
                .unwrap();
            let old = witness.ledger.as_ref().unwrap();
            fs::write(&witness.source.path, b"fixture\nappend\n").unwrap();
            let bytes = b"fixture\nappend\n";
            let ledger = Ledger {
                stat: Stat::of(&witness.source.path).unwrap(),
                offset: bytes.len() as u64,
                prefix: Sha256::digest(bytes).into(),
                tail: Sha256::digest(bytes).into(),
            };
            let changes = super::super::super::super::DirtyRows::default();
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            write_index_changes(
                &tx,
                witness.source.path.to_str().unwrap(),
                &ledger,
                &fixture.index,
                Some(&changes),
                Some(old),
            )
            .unwrap();
            tx.commit().unwrap();
            let mut source = ledger.source(&witness.source.path, "current");
            source.native_role = witness.source.native_role.clone();
            let appended = fixture.store.source_witness(&source).unwrap();
            let (reused, p, work) = fixture
                .store
                .physical_profile_current_step(&appended, &|| false)
                .unwrap();
            assert_eq!(reused, root);
            assert_eq!(p, ProfileProgress::Ready);
            assert_eq!(work.decoded_rows, 0);
            assert_eq!(work.metadata_bytes, 0);
            assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row(
                        "SELECT atoms FROM native_physical_counts WHERE root=?1",
                        [root.0.as_slice()],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                before_atoms
            );
            assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row("SELECT count(*) FROM native_original_repairs", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            let mut changed = fixture.index.clone();
            changed.links.session_ids.insert("new".into());
            let mut changes = changes;
            changes.links.insert(("session".into(), "new".into()));
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            capture_revision(
                &tx,
                witness.source.path.to_str().unwrap(),
                Some(&changes),
                Some(&changed),
            )
            .unwrap();
            tx.commit().unwrap();
            let (changed_root, _, _) = fixture
                .store
                .physical_profile_current_step(&appended, &|| false)
                .unwrap();
            assert_ne!(changed_root, root);
        }
    }

    #[test]
    fn escaped_native_fragments_resume_without_a_full_decoded_part_per_tick() {
        for length in [0, PART, PART * 2 + 17] {
            let mut fixture = Fixture::new("fragments", &[], &[]);
            let witness = fixture.witness.clone();
            let job = fixture.admit_source(&witness);
            let repair = fixture.repair(&job, &witness);
            let sources = crate::query_source::Sources::capture(
                &fixture.store.options,
                std::iter::once((witness.source.clone(), None)),
            )
            .unwrap();
            let records = sources.reader();
            let operation = OriginalProfileOperation {
                job: &job,
                repair: &repair,
                records: &records,
            };
            let (root, _) = fixture
                .store
                .physical_profile_original_target(
                    operation,
                    &witness,
                    None,
                    PhysicalRole::Top,
                    NativeScanOrder::new(PhysicalRole::Top, &witness.source.path),
                )
                .unwrap();
            fixture
                .store
                .physical_profile_original_fragment(
                    operation,
                    &witness,
                    root,
                    0,
                    0,
                    AtomKind::Own,
                    b"fragments",
                    true,
                )
                .unwrap();
            assert!(
                fixture
                    .store
                    .physical_profile_original_fragment_advance(operation, &witness, root, 0)
                    .unwrap()
                    .0
                    .drained
            );
            while fixture
                .store
                .physical_profile_original_advance(operation, &witness, root, &|| false)
                .unwrap()
                .0
                == ProfileProgress::Pending
            {}
            let atom = vec![b'x'; length];
            let mut offset = 0;
            for fragment in atom.chunks(21000) {
                let received = fixture
                    .store
                    .physical_profile_original_fragment(
                        operation,
                        &witness,
                        root,
                        1,
                        offset as u64,
                        AtomKind::Session,
                        fragment,
                        false,
                    )
                    .unwrap();
                assert!(!received.drained);
                fixture.store = NativeStore::open(&fixture.store.options).unwrap();
                let replay = fixture
                    .store
                    .physical_profile_original_fragment(
                        operation,
                        &witness,
                        root,
                        1,
                        offset as u64,
                        AtomKind::Session,
                        fragment,
                        false,
                    )
                    .unwrap();
                assert_eq!(replay.next_offset, received.next_offset);
                // Simulate losing the carry CAS after a canonical part commit.
                let before = offset / PART;
                let after = (offset + fragment.len()) / PART;
                if after > before {
                    fixture
                        .store
                        .physical_profile_original_part(
                            operation,
                            &witness,
                            root,
                            1,
                            (before * PART) as u64,
                            AtomKind::Session,
                            &atom[before * PART..(before + 1) * PART],
                            false,
                        )
                        .unwrap();
                }
                let (drained, work) = fixture
                    .store
                    .physical_profile_original_fragment_advance(operation, &witness, root, 1)
                    .unwrap();
                assert!(drained.drained);
                assert_eq!(drained.next_offset, received.next_offset);
                assert!(work.metadata_bytes <= PART * 2 + 128);
                assert!(
                    fixture
                        .store
                        .physical_profile_original_finish(operation, &witness, root, 1)
                        .is_err()
                );
                offset += fragment.len();
            }
            fixture
                .store
                .physical_profile_original_fragment(
                    operation,
                    &witness,
                    root,
                    1,
                    offset as u64,
                    AtomKind::Session,
                    &[],
                    true,
                )
                .unwrap();
            let mut drained = false;
            for _ in 0..2 {
                if fixture
                    .store
                    .physical_profile_original_fragment_advance(operation, &witness, root, 1)
                    .unwrap()
                    .0
                    .drained
                {
                    drained = true;
                    break;
                }
            }
            assert!(drained);
            while fixture
                .store
                .physical_profile_original_advance(operation, &witness, root, &|| false)
                .unwrap()
                .0
                == ProfileProgress::Pending
            {}
            fixture
                .store
                .physical_profile_original_finish(operation, &witness, root, 2)
                .unwrap();
            let mut rebuilt = Vec::new();
            let atom_id: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT atom FROM native_physical_atoms WHERE root=?1 AND ordinal=1",
                    [root.0.as_slice()],
                    |r| r.get(0),
                )
                .unwrap();
            for i in 0..length.div_ceil(PART).max(1) {
                rebuilt.extend(
                    part(
                        &fixture.store.connection,
                        atom_id,
                        i as i64,
                        &mut Work::default(),
                    )
                    .unwrap(),
                );
            }
            assert_eq!(rebuilt, atom);
            if length > 0 {
                assert!(
                    fixture
                        .store
                        .physical_profile_original_fragment(
                            operation,
                            &witness,
                            root,
                            1,
                            0,
                            AtomKind::Session,
                            &atom[..atom.len().min(21000)],
                            false
                        )
                        .unwrap()
                        .drained
                );
                assert!(
                    fixture
                        .store
                        .physical_profile_original_fragment(
                            operation,
                            &witness,
                            root,
                            1,
                            0,
                            AtomKind::Session,
                            b"changed",
                            false
                        )
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn old_missing_bootstrap_footer_queues_native_atoms_without_scanning_links() {
        let fixture = Fixture::new("old", &["small"], &[]);
        fixture
            .store
            .connection
            .execute("DELETE FROM native_physical_bootstrap", [])
            .unwrap();
        let mut required = false;
        for _ in 0..16 {
            let (_, p, _) = fixture
                .store
                .physical_profile_current_step(&fixture.witness, &|| false)
                .unwrap();
            if p == ProfileProgress::NativeAtomsRequired {
                required = true;
                break;
            }
        }
        assert!(required);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_physical_current_cursors",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn skipped_context_is_paged_durable_and_foreground_read_is_scalar() {
        let mut fixture = Fixture::new("paged", &["needle"], &[]);
        let root = fixture.prepare();
        for count in [0usize, 128, 1024, 8192] {
            let mut skipped = (0..count)
                .map(|i| format!("skip-{i:05}"))
                .collect::<Vec<_>>();
            if let Some(last) = skipped.last_mut() {
                *last = "needle".into();
            }
            let context = digest(&count.to_le_bytes());
            assert!(
                fixture
                    .store
                    .physical_profile_for_context(&fixture.witness, context)
                    .is_err()
            );
            let mut after = 0;
            let mut answer = None;
            for tick in 0..count / 8 + 2 {
                let end = (after + ROWS).min(skipped.len());
                let (progress, work) = fixture
                    .store
                    .physical_truncation_step(
                        root,
                        context,
                        after as u64,
                        &skipped[after..end],
                        end == skipped.len(),
                    )
                    .unwrap();
                assert!(work.decoded_rows <= ROWS - 40);
                assert!(work.metadata_bytes <= STATE_BYTES);
                assert!(progress.next >= after as u64 && progress.next <= after as u64 + 8);
                if tick == 0 {
                    fixture.store = NativeStore::open(&fixture.store.options).unwrap();
                }
                after = progress.next as usize;
                if progress.answer.is_some() {
                    answer = progress.answer;
                    break;
                }
            }
            assert_eq!(answer, Some(count > 0));
            let before = fixture.store.connection.total_changes();
            let profile = fixture
                .store
                .physical_profile_for_context(&fixture.witness, context)
                .unwrap();
            assert_eq!(profile.turns_truncated, count > 0);
            assert_eq!(fixture.store.connection.total_changes(), before);
        }
    }

    #[test]
    fn original_feed_preserves_absent_or_newer_current_and_rechecks_observations() {
        for absent in [true, false] {
            let mut fixture = Fixture::new("original", &[], &[]);
            let source = fixture.witness.source.clone();
            if absent {
                fixture
                    .store
                    .connection
                    .execute(
                        "DELETE FROM files WHERE path=?1",
                        [source.path.to_str().unwrap()],
                    )
                    .unwrap();
            } else {
                fixture.store.connection.execute("UPDATE files SET size=size+8,resume_at=resume_at+8,head_sha256=?2,tail_sha256=?2 WHERE path=?1",params![source.path.to_str(),[7u8;32].as_slice()]).unwrap();
            }
            let witness = fixture.store.original_source_witness(&source).unwrap();
            assert_eq!(witness.ledger.is_none(), absent);
            assert!(fixture.store.source_witness_current(&witness).unwrap());
            let spec = JobSpec {
                view_key: format!("original-profile-{absent}"),
                version: 1,
                catalog_generation: None,
                runtime: crate::native_evidence::query_runtime_generation(&fixture.store.options)
                    .unwrap(),
            };
            let job = fixture.store.begin(&spec).unwrap();
            assert_eq!(
                fixture
                    .store
                    .admit(&job, std::slice::from_ref(&witness), &[], "profile")
                    .unwrap(),
                Outcome::Written
            );
            let job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
            let repair = fixture.repair(&job, &witness);
            let sources = crate::query_source::Sources::capture(
                &fixture.store.options,
                std::iter::once((witness.source.clone(), None)),
            )
            .unwrap();
            let records = sources.reader();
            let operation = OriginalProfileOperation {
                job: &job,
                repair: &repair,
                records: &records,
            };
            let (root, progress) = fixture
                .store
                .physical_profile_original_target(
                    operation,
                    &witness,
                    Some(-9),
                    PhysicalRole::Top,
                    NativeScanOrder::new(PhysicalRole::Top, &source.path),
                )
                .unwrap();
            assert_eq!(
                progress,
                ProfileProgress::Input {
                    ordinal: 0,
                    offset: 0
                }
            );
            assert!(fixture.store.physical_profile(&witness, &[]).is_err());
            fixture
                .store
                .physical_profile_original_fragment(
                    operation,
                    &witness,
                    root,
                    0,
                    0,
                    AtomKind::Own,
                    b"original",
                    true,
                )
                .unwrap();
            assert!(
                fixture
                    .store
                    .physical_profile_original_fragment_advance(operation, &witness, root, 0)
                    .unwrap()
                    .0
                    .drained
            );
            while fixture
                .store
                .physical_profile_original_advance(operation, &witness, root, &|| false)
                .unwrap()
                .0
                == ProfileProgress::Pending
            {}
            fixture
                .store
                .physical_profile_original_fragment(
                    operation,
                    &witness,
                    root,
                    1,
                    0,
                    AtomKind::Session,
                    b"skipped",
                    true,
                )
                .unwrap();
            assert!(
                fixture
                    .store
                    .physical_profile_original_fragment_advance(operation, &witness, root, 1)
                    .unwrap()
                    .0
                    .drained
            );
            while fixture
                .store
                .physical_profile_original_advance(operation, &witness, root, &|| false)
                .unwrap()
                .0
                == ProfileProgress::Pending
            {}
            fixture
                .store
                .physical_profile_original_finish(operation, &witness, root, 2)
                .unwrap();
            let profile = fixture
                .store
                .physical_profile(&witness, &["skipped".into()])
                .unwrap();
            assert_eq!(profile.first, Some(-9));
            assert!(profile.turns_truncated);
            fixture
                .store
                .connection
                .execute(
                    "UPDATE native_original_repairs SET revision=revision+1 WHERE original=?1",
                    [repair.original.as_slice()],
                )
                .unwrap();
            assert!(
                fixture
                    .store
                    .physical_profile_original_finish(operation, &witness, root, 2)
                    .is_err()
            );
            // Completed pure source facts remain reusable; only the stale
            // extraction operation loses its qualification to mutate/seal.
            assert!(
                fixture
                    .store
                    .physical_profile(&witness, &["skipped".into()])
                    .unwrap()
                    .turns_truncated
            );
            fixture
                .store
                .connection
                .execute(
                    "UPDATE native_query_jobs SET revision=revision+1 WHERE job=?1",
                    [job.id],
                )
                .unwrap();
            assert!(
                fixture
                    .store
                    .physical_profile_original_fragment(
                        operation,
                        &witness,
                        root,
                        0,
                        0,
                        AtomKind::Own,
                        b"original",
                        true
                    )
                    .is_err()
            );
            // Observation changes invalidate every feed entry point even
            // though the immutable Original source proof remains unchanged.
            fixture.store.connection.execute("INSERT INTO meta(key,value) VALUES(?1,'changed') ON CONFLICT(key) DO UPDATE SET value=excluded.value",[archive_index_key(&source.path.to_string_lossy())]).unwrap();
            assert!(
                fixture
                    .store
                    .physical_profile_original_advance(operation, &witness, root, &|| false)
                    .is_err()
            );
            assert!(
                fixture
                    .store
                    .physical_profile_original_finish(operation, &witness, root, 2)
                    .is_err()
            );
            assert!(fixture.store.physical_profile(&witness, &[]).is_err());
        }
    }

    #[test]
    fn descriptor_atom_count_and_required_parts_refuse_corrupt_ready_metadata() {
        let fixture = Fixture::new("chosen", &[], &[]);
        let root = fixture.prepare();
        fixture.store.connection.execute("UPDATE native_physical_profiles SET descriptor=json_set(descriptor,'$.native_id','wrong') WHERE root=?1",[root.0.as_slice()]).unwrap();
        assert!(
            fixture
                .store
                .physical_profile(&fixture.witness, &[])
                .is_err()
        );
        let fixture = Fixture::new("chosen", &[], &[]);
        let root = fixture.prepare();
        let atom: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT minimum FROM native_physical_profiles WHERE root=?1",
                [root.0.as_slice()],
                |r| r.get(0),
            )
            .unwrap();
        fixture
            .store
            .connection
            .execute("DELETE FROM native_physical_atoms WHERE atom=?1", [atom])
            .unwrap();
        assert!(
            fixture
                .store
                .physical_profile_finish(&fixture.witness, root, 1)
                .is_err()
        );
        assert!(
            fixture
                .store
                .physical_profile(&fixture.witness, &[])
                .is_err()
        );
        let fixture = Fixture::new("chosen", &[], &[]);
        let root = fixture.prepare();
        let atom: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT minimum FROM native_physical_profiles WHERE root=?1",
                [root.0.as_slice()],
                |r| r.get(0),
            )
            .unwrap();
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_physical_atom_parts SET bytes=X'78' WHERE atom=?1 AND part=0",
                [atom],
            )
            .unwrap();
        assert!(part(&fixture.store.connection, atom, 0, &mut Work::default()).is_err());
    }

    #[test]
    fn published_pairs_preserve_phantoms_literal_bridge_and_omitted_connectors() {
        let fixture = Fixture::new("a", &["b", "phantom"], &[]);
        let a = fixture.prepare();
        let b = fixture.prepare_source(&fixture.add_source("b", &["c"], &[]));
        let c = fixture.prepare_source(&fixture.add_source("c", &[], &[]));
        let x = fixture.prepare_source(&fixture.add_source("x", &["phantom"], &[]));
        let y = fixture.prepare_source(&fixture.add_source("y", &["bridge:literal"], &[]));
        let z = fixture.prepare_source(&fixture.add_source("z", &[], &["literal"]));
        assert!(fixture.store.physical_pair(a, b).unwrap());
        assert!(fixture.store.physical_pair(b, c).unwrap());
        assert!(!fixture.store.physical_pair(a, c).unwrap());
        assert!(fixture.store.physical_pair(a, x).unwrap());
        assert!(fixture.store.physical_pair(y, z).unwrap());
        assert!(!fixture.store.physical_pair(c, z).unwrap());
        assert!(fixture.store.physical_minimum_order(a, c).is_err());
        while fixture
            .store
            .physical_minimum_order_advance(a, c)
            .unwrap()
            .0
            .is_none()
        {}
        assert_eq!(
            fixture.store.physical_minimum_order(a, c).unwrap(),
            Ordering::Less
        );
    }

    #[test]
    fn actual_current_profile_reopens_without_raw_link_rescan_and_session_only_truncates() {
        let mut fixture = Fixture::new("chosen", &["skipped"], &["other"]);
        let (root, _, _) = fixture
            .store
            .physical_profile_current_step(&fixture.witness, &|| false)
            .unwrap();
        let reopened = NativeStore::open(&fixture.store.options).unwrap();
        fixture.store = reopened;
        assert_eq!(fixture.prepare(), root);
        let profile = fixture
            .store
            .physical_profile(&fixture.witness, &["skipped".into()])
            .unwrap();
        assert!(profile.turns_truncated);
        assert!(
            !fixture
                .store
                .physical_profile(&fixture.witness, &["other".into()])
                .unwrap()
                .turns_truncated
        );
        let reads = Arc::new(AtomicU64::new(0));
        let measured = Arc::clone(&reads);
        fixture.store.connection.progress_handler(
            1,
            Some(move || {
                measured.fetch_add(1, AtomicOrdering::Relaxed);
                false
            }),
        );
        let (_, progress, work) = fixture
            .store
            .physical_profile_current_step(&fixture.witness, &|| false)
            .unwrap();
        fixture
            .store
            .connection
            .progress_handler(0, None::<fn() -> bool>);
        assert_eq!(progress, ProfileProgress::Ready);
        assert_eq!(work.decoded_rows, 0);
        assert!(reads.load(AtomicOrdering::Relaxed) > 0);
    }

    #[test]
    fn event_only_or_unchanged_touched_links_reuse_revision_and_profiles() {
        let fixture = Fixture::new("chosen", &["linked"], &[]);
        let root = fixture.prepare();
        let mut changes = super::super::super::super::DirtyRows::default();
        let path = fixture.witness.source.path.to_str().unwrap();
        for _ in 0..4 {
            capture_revision(
                &fixture.store.connection,
                path,
                Some(&changes),
                Some(&fixture.index),
            )
            .unwrap();
            assert_eq!(fixture.prepare(), root);
            changes.links.insert(("session".into(), "linked".into()));
        }
        changes.links.insert(("session".into(), "new".into()));
        let mut changed = fixture.index.clone();
        changed.links.session_ids.insert("new".into());
        capture_revision(
            &fixture.store.connection,
            path,
            Some(&changes),
            Some(&changed),
        )
        .unwrap();
        assert!(
            fixture
                .store
                .physical_profile_part(&fixture.witness, root, 0, 0, AtomKind::Own, b"chosen", true)
                .is_err()
        );
    }

    #[test]
    fn giant_atom_parts_resume_minimum_and_exact_collision_comparison() {
        let fixture = Fixture::new("zz-owner", &[], &[]);
        let role = PhysicalRole::Top;
        let (root, _) = fixture
            .store
            .physical_profile_begin(
                &fixture.witness,
                role,
                NativeScanOrder::new(role, &fixture.witness.source.path),
            )
            .unwrap();
        fixture
            .store
            .physical_profile_part(
                &fixture.witness,
                root,
                0,
                0,
                AtomKind::Own,
                b"zz-owner",
                true,
            )
            .unwrap();
        while fixture
            .store
            .physical_profile_advance(&fixture.witness, root, &|| false)
            .unwrap()
            .0
            == ProfileProgress::Pending
        {}
        let block = vec![b'a'; PART];
        for i in 0..20 {
            let p = fixture
                .store
                .physical_profile_part(
                    &fixture.witness,
                    root,
                    1,
                    (i * PART) as u64,
                    AtomKind::Session,
                    &block,
                    i == 19,
                )
                .unwrap();
            assert_eq!(
                p,
                if i == 19 {
                    ProfileProgress::Pending
                } else {
                    ProfileProgress::Input {
                        ordinal: 1,
                        offset: ((i + 1) * PART) as u64,
                    }
                }
            );
        }
        loop {
            let (p, work) = fixture
                .store
                .physical_profile_advance(&fixture.witness, root, &|| false)
                .unwrap();
            assert!(work.metadata_bytes <= 2 * PART);
            if matches!(p, ProfileProgress::Input { .. }) {
                break;
            }
        }
        fixture
            .store
            .physical_profile_finish(&fixture.witness, root, 2)
            .unwrap();
        let minimum: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT minimum FROM native_physical_profiles WHERE root=?1",
                [root.0.as_slice()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            atom_info(&fixture.store.connection, minimum).unwrap().0,
            20 * PART as i64
        );
        assert_eq!(
            compare_part(
                &fixture.store.connection,
                minimum,
                minimum,
                20,
                &mut Work::default()
            )
            .unwrap(),
            Some(Ordering::Equal)
        );
        fixture
            .store
            .connection
            .execute(
                "DELETE FROM native_physical_atom_parts WHERE atom=?1 AND part=10",
                [minimum],
            )
            .unwrap();
        assert!(
            compare_part(
                &fixture.store.connection,
                minimum,
                minimum,
                10,
                &mut Work::default()
            )
            .is_err()
        );
        assert!(
            fixture
                .store
                .physical_profile(&fixture.witness, &[])
                .is_err()
        );
        fixture
            .store
            .connection
            .execute(
                "INSERT INTO native_physical_atom_parts VALUES(?1,10,?2)",
                params![minimum, block],
            )
            .unwrap();
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_physical_atom_parts SET bytes=X'61' WHERE atom=?1 AND part=10",
                [minimum],
            )
            .unwrap();
        assert!(
            compare_part(
                &fixture.store.connection,
                minimum,
                minimum,
                10,
                &mut Work::default()
            )
            .is_err()
        );
        assert_eq!(
            fixture
                .store
                .physical_profile_advance(&fixture.witness, root, &|| true)
                .unwrap()
                .0,
            ProfileProgress::Pending
        );
        // Coarse digest equality never establishes equality by itself.
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute("INSERT INTO native_physical_atoms VALUES(1,zeroblob(32),0,'own',1,zeroblob(32),1),(2,randomblob(32),0,'own',1,zeroblob(32),1)",[]).unwrap();
        db.execute(
            "INSERT INTO native_physical_atom_parts VALUES(1,0,X'61'),(2,0,X'62')",
            [],
        )
        .unwrap();
        assert!(compare_part(&db, 1, 2, 0, &mut Work::default()).is_err());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AtomKind {
    Own,
    Session,
    Continued,
    Bridge,
}
impl AtomKind {
    fn name(self) -> &'static str {
        match self {
            Self::Own => "own",
            Self::Session => "session",
            Self::Continued => "continued",
            Self::Bridge => "bridge",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProfileProgress {
    /// The exact native producer supplies the next atom in canonical 64KiB
    /// parts. A final empty part is permitted only for an empty atom.
    Input {
        ordinal: u64,
        offset: u64,
    },
    Pending,
    /// The same native producer must stream qualified parts from Original
    /// evidence. This is durable repair interest, never a permanent refusal.
    NativeAtomsRequired,
    Ready,
}

#[derive(Clone, Copy)]
pub(crate) struct OriginalProfileOperation<'a, 'r> {
    pub(crate) job: &'a Job,
    pub(crate) repair: &'a super::native_repair::Repair,
    pub(crate) records: &'a crate::query_source::Records<'r>,
}
#[derive(Clone, Debug)]
pub(crate) struct TruncationProgress {
    pub(crate) next: u64,
    pub(crate) answer: Option<bool>,
}
/// `next_offset` is the decoded native atom cursor, independent of canonical
/// stored-part length. Drain before feeding another fragment or another atom.
#[derive(Clone, Debug)]
pub(crate) struct FragmentProgress {
    pub(crate) next_offset: u64,
    pub(crate) drained: bool,
    pub(crate) profile: ProfileProgress,
}
/// Conservative admission reservation, NOT an actual decoded-work receipt.
/// `Work` returned by this module covers atom/page payload only. Charge this
/// controller scope separately, measure whole-call connection VM/allocation,
/// and include actual native source/Original admission closure receipts. In
/// particular a cached Ready result has zero atom payload, never zero total IO.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProfileControllerReservation {
    pub(crate) scalar_rows: usize,
    pub(crate) scalar_bytes: usize,
    /// Additional bounded BLOB reads/writes (receipt replay, canonical publish
    /// and carry CAS) that are not returned as selected payload observations.
    /// Their row operations are already included in `scalar_rows`.
    pub(crate) additional_payload_bytes: usize,
    pub(crate) source_checks: usize,
    /// Each admission invokes the native owner's complete job/witness rules;
    /// its closure is additional work, not bounded by `source_checks` here.
    pub(crate) original_admissions: usize,
    pub(crate) runtime_checks: usize,
}
fn descriptor_bound(witness: &SourceWitness) -> usize {
    witness
        .source
        .native_id
        .len()
        .saturating_add(witness.source.path.as_os_str().len())
        .saturating_mul(6)
        .saturating_add(256)
}
pub(crate) fn controller_reservation(
    witness: Option<&SourceWitness>,
    original: bool,
) -> ProfileControllerReservation {
    ProfileControllerReservation {
        scalar_rows: 40,
        scalar_bytes: witness
            .map(descriptor_bound)
            .unwrap_or(0)
            .saturating_mul(4)
            .saturating_add(8192),
        additional_payload_bytes: PART * 4,
        source_checks: if witness.is_some() { 24 } else { 0 },
        original_admissions: if original { 8 } else { 0 },
        runtime_checks: if original { 8 } else { 0 },
    }
}
fn operation_current(
    db: &Connection,
    witness: &SourceWitness,
    root: ProfileRoot,
    operation: Option<OriginalProfileOperation<'_, '_>>,
    complete: bool,
) -> io::Result<()> {
    if let Some(operation) = operation {
        super::native_repair::physical_profile_admitted(
            db,
            operation.job,
            witness,
            operation.repair,
            operation.records,
            complete,
        )
        .map_err(io::Error::other)?;
    } else {
        let original: bool = db
            .query_row(
                "SELECT original IS NOT NULL FROM native_physical_profiles WHERE root=?1",
                [root.0.as_slice()],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        if original {
            return Err(io::Error::other(
                "Original physical operation requires qualified native repair",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    native_id: String,
    role: PhysicalRole,
    scan_order: NativeScanOrder,
}
#[derive(Debug)]
struct State {
    phase: String,
    next: i64,
    atom: Option<i64>,
    minimum: Option<i64>,
    compare: Option<i64>,
    part: i64,
    peer_after: i64,
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn source_key(source: &CatalogSource) -> io::Result<[u8; 32]> {
    Ok(digest(
        &serde_json::to_vec(source).map_err(io::Error::other)?,
    ))
}
fn state(db: &Connection, root: ProfileRoot) -> rusqlite::Result<State> {
    db.query_row("SELECT CASE WHEN phase IN ('input','minimum','peers','ready') THEN phase END,next_atom,atom,minimum,compare_atom,compare_part,peer_after FROM native_physical_profiles WHERE root=?1 AND next_atom>=0 AND compare_part>=0 AND peer_after>=0", [root.0.as_slice()], |r| Ok(State {
        phase:r.get(0)?, next:r.get(1)?, atom:r.get(2)?, minimum:r.get(3)?, compare:r.get(4)?, part:r.get(5)?, peer_after:r.get(6)?,
    }))
}
fn ensure_build(
    db: &Connection,
    witness: &SourceWitness,
    root: ProfileRoot,
) -> rusqlite::Result<()> {
    if !check_source(db, witness)? {
        return Err(invalid("physical profile source changed"));
    }
    let original: Option<Vec<u8>> = db.query_row(
        "SELECT CASE WHEN original IS NULL OR octet_length(original)=32 THEN original ELSE zeroblob(0) END FROM native_physical_profiles WHERE root=?1",
        [root.0.as_slice()],
        |r| r.get(0),
    )?;
    if let Some(original) = original {
        if original.as_slice()
            != source_key(&witness.source)
                .map_err(|_| invalid("invalid Original physical proof"))?
        {
            return Err(invalid("Original physical source differs"));
        }
        return Ok(());
    }
    let current = current_file(db, witness)?;
    let _ = current;
    let admitted:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM native_physical_profiles p JOIN native_physical_link_revisions r ON r.path=p.path AND r.revision=p.revision WHERE p.root=?1 AND p.path=?2)",params![root.0.as_slice(),witness.source.path.to_str()],|r|r.get(0))?;
    if !admitted {
        return Err(invalid("physical profile link revision changed"));
    }
    Ok(())
}
fn complete_counts(db: &Connection, root: ProfileRoot, expected: i64) -> rusqlite::Result<bool> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM native_physical_counts WHERE root=?1 AND atoms=?2 AND complete=?2 AND expected_parts=parts AND parts=hashes) AND NOT EXISTS(SELECT 1 FROM native_physical_carry WHERE root=?1)",params![root.0.as_slice(),expected],|r|r.get(0))
}
fn ready_state(db: &Connection, root: ProfileRoot) -> io::Result<State> {
    let s = state(db, root).map_err(io::Error::other)?;
    let minimum:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM native_physical_atoms WHERE atom=?2 AND root=?1 AND complete=1)",params![root.0.as_slice(),s.minimum],|r|r.get(0)).map_err(io::Error::other)?;
    if s.phase != "ready"
        || !minimum
        || !complete_counts(db, root, s.next).map_err(io::Error::other)?
    {
        return Err(io::Error::other(
            "physical profile completion evidence changed",
        ));
    }
    Ok(s)
}
fn current_root(path: &str, revision: i64, descriptor: &str) -> io::Result<ProfileRoot> {
    Ok(ProfileRoot(digest(
        &serde_json::to_vec(&("physical-profile-v2", path, revision, descriptor))
            .map_err(io::Error::other)?,
    )))
}
fn atom_info(db: &Connection, atom: i64) -> rusqlite::Result<(i64, Vec<u8>, bool)> {
    db.query_row(
        "SELECT length,CASE WHEN octet_length(digest)=32 THEN digest END,complete FROM native_physical_atoms WHERE atom=?1 AND length>=0 AND ordinal>=0 AND complete IN (0,1) AND kind IN ('own','session','continued','bridge') AND octet_length(root)=32",
        [atom],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
}
fn part(db: &Connection, atom: i64, index: i64, work: &mut Work) -> io::Result<Vec<u8>> {
    let (length, final_digest, complete) = atom_info(db, atom).map_err(io::Error::other)?;
    let offset = index
        .checked_mul(PART as i64)
        .filter(|offset| *offset >= 0)
        .ok_or_else(|| io::Error::other("invalid physical part index"))?;
    if length > 0 && offset == length && complete && length % PART as i64 == 0 {
        let valid:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM native_physical_atom_parts p JOIN native_physical_part_hashes h ON h.atom=p.atom AND h.part=p.part WHERE p.atom=?1 AND p.part=?2 AND octet_length(p.bytes)=65536 AND h.digest=?3)",params![atom,index-1,final_digest],|r|r.get(0)).map_err(io::Error::other)?;
        if !valid {
            return Err(io::Error::other(
                "physical EOF lacks its complete final part",
            ));
        }
        return Ok(Vec::new());
    }
    if offset > length || (offset == length && length != 0) {
        return Err(io::Error::other(
            "physical part exceeds declared atom length",
        ));
    }
    let expected = (length - offset).min(PART as i64);
    let bytes: Option<Vec<u8>> = db
        .query_row(
            "SELECT CASE WHEN octet_length(bytes)=?3 AND octet_length(bytes)<=65536 THEN bytes END FROM native_physical_atom_parts WHERE atom=?1 AND part=?2",
            params![atom, index,expected],
            |r| r.get(0),
        )
        .optional()
        .map_err(io::Error::other)?;
    work.decoded_rows += usize::from(bytes.is_some());
    let bytes =
        bytes.ok_or_else(|| io::Error::other("physical atom is missing a required part"))?;
    let stored:Vec<u8>=db.query_row("SELECT CASE WHEN octet_length(digest)=32 THEN digest END FROM native_physical_part_hashes WHERE atom=?1 AND part=?2",params![atom,index],|r|r.get(0)).map_err(io::Error::other)?;
    let previous = if index == 0 {
        vec![0u8; 32]
    } else {
        db.query_row("SELECT CASE WHEN octet_length(digest)=32 THEN digest END FROM native_physical_part_hashes WHERE atom=?1 AND part=?2",params![atom,index-1],|r|r.get::<_,Vec<u8>>(0)).map_err(io::Error::other)?
    };
    let mut hash = Sha256::new();
    hash.update(previous);
    hash.update(&bytes);
    let actual: [u8; 32] = hash.finalize().into();
    if stored != actual || (complete && offset + expected == length && final_digest != actual) {
        return Err(io::Error::other("physical atom part digest changed"));
    }
    work.metadata_bytes += bytes.len();
    Ok(bytes)
}
/// Compare one canonical part. None means equal so far, with more parts.
fn compare_part(
    db: &Connection,
    a: i64,
    b: i64,
    index: i64,
    work: &mut Work,
) -> io::Result<Option<Ordering>> {
    let a = part(db, a, index, work)?;
    let b = part(db, b, index, work)?;
    let order = a.cmp(&b);
    Ok(if order.is_eq() && a.len() == PART {
        None
    } else {
        Some(order)
    })
}
fn progress(db: &Connection, root: ProfileRoot) -> io::Result<ProfileProgress> {
    let s = state(db, root).map_err(io::Error::other)?;
    Ok(match s.phase.as_str() {
        "ready" => ProfileProgress::Ready,
        "input" => ProfileProgress::Input {
            ordinal: u64::try_from(s.next).map_err(io::Error::other)?,
            offset: match s.atom {
                Some(atom) => u64::try_from(atom_info(db, atom).map_err(io::Error::other)?.0)
                    .map_err(io::Error::other)?,
                None => 0,
            },
        },
        _ => ProfileProgress::Pending,
    })
}

impl NativeStore {
    fn physical_operation_after(
        &self,
        witness: &SourceWitness,
        operation: Option<OriginalProfileOperation<'_, '_>>,
        complete: bool,
    ) -> io::Result<()> {
        if let Some(operation) = operation {
            if !self.runtime_current(&operation.job.spec)? {
                return Err(io::Error::other(
                    "Original physical runtime changed after metadata",
                ));
            }
            super::native_repair::physical_profile_admitted(
                &self.connection,
                operation.job,
                witness,
                operation.repair,
                operation.records,
                complete,
            )
            .map_err(io::Error::other)?;
        }
        if !self.source_witness_current(witness)? {
            return Err(io::Error::other("physical source changed after metadata"));
        }
        Ok(())
    }
    /// Original native evidence owns extraction and scalar semantics. Its
    /// existing job/source admission qualifies this exact immutable source;
    /// the separate Current ledger may be absent or newer throughout repair.
    pub(crate) fn physical_profile_original_target(
        &self,
        operation: OriginalProfileOperation<'_, '_>,
        witness: &SourceWitness,
        first: Option<i64>,
        role: PhysicalRole,
        scan_order: NativeScanOrder,
    ) -> io::Result<(ProfileRoot, ProfileProgress)> {
        let job = operation.job;
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("Original physical runtime changed"));
        }
        if PhysicalRole::recorded(&witness.source).is_some_and(|recorded| recorded != role)
            || scan_order != NativeScanOrder::new(role, &witness.source.path)
        {
            return Err(io::Error::other(
                "Original physical role or scan order changed",
            ));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        super::native_repair::physical_profile_admitted(
            &tx,
            job,
            witness,
            operation.repair,
            operation.records,
            false,
        )
        .map_err(io::Error::other)?;
        let source = source_key(&witness.source)?;
        let admitted:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_query_sources WHERE job=?1 AND path=?2 AND metadata=?3)",params![job.id,witness.source.path.to_str(),json(&witness.source).map_err(io::Error::other)?],|r|r.get(0)).map_err(io::Error::other)?;
        if !admitted {
            return Err(io::Error::other("Original physical source not admitted"));
        }
        let descriptor = serde_json::to_string(&Descriptor {
            native_id: witness.source.native_id.clone(),
            role,
            scan_order,
        })
        .map_err(io::Error::other)?;
        let current = witness
            .ledger
            .as_ref()
            .is_some_and(|ledger| source_matches(&witness.source, ledger));
        let path = witness
            .source
            .path
            .to_str()
            .ok_or_else(|| io::Error::other("invalid Original physical path"))?;
        let (root, revision, original) = if current {
            tx.execute(
                "INSERT OR IGNORE INTO native_physical_link_revisions VALUES(?1,1)",
                [path],
            )
            .map_err(io::Error::other)?;
            let revision: i64 = tx
                .query_row(
                    "SELECT revision FROM native_physical_link_revisions WHERE path=?1",
                    [path],
                    |r| r.get(0),
                )
                .map_err(io::Error::other)?;
            (current_root(path, revision, &descriptor)?, revision, None)
        } else {
            (
                ProfileRoot(digest(
                    &serde_json::to_vec(&("physical-original-v2", source, &descriptor))
                        .map_err(io::Error::other)?,
                )),
                0,
                Some(source),
            )
        };
        tx.execute("INSERT OR IGNORE INTO native_physical_profiles(root,path,revision,descriptor,phase,original) VALUES(?1,?2,?3,?4,'input',?5)",params![root.0.as_slice(),path,revision,descriptor,original.as_ref().map(|key|key.as_slice())]).map_err(io::Error::other)?;
        tx.execute("INSERT INTO native_physical_bindings VALUES(?1,?2,?3) ON CONFLICT(source) DO UPDATE SET root=excluded.root,first_ms=excluded.first_ms",params![source.as_slice(),root.0.as_slice(),first]).map_err(io::Error::other)?;
        let progress = progress(&tx, root)?;
        super::native_repair::physical_profile_admitted(
            &tx,
            job,
            witness,
            operation.repair,
            operation.records,
            false,
        )
        .map_err(io::Error::other)?;
        tx.commit().map_err(io::Error::other)?;
        self.physical_operation_after(witness, Some(operation), false)?;
        if !self.runtime_current(&job.spec)?
            || !self.source_witness_current(witness)?
            || !job_current(&self.connection, job).map_err(io::Error::other)?
        {
            return Err(io::Error::other(
                "Original physical source changed after begin",
            ));
        }
        Ok((root, progress))
    }
    /// Ordinary Current facts use the already bounded native primitive reader.
    /// Larger atoms hand the SAME durable root to the Original native producer;
    /// they never trigger an unbounded SQL string copy or an Index fallback.
    pub(crate) fn physical_profile_current_step(
        &self,
        witness: &SourceWitness,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<(ProfileRoot, ProfileProgress, Work)> {
        let role = PhysicalRole::recorded(&witness.source)
            .ok_or_else(|| io::Error::other("physical native role requires bounded repair"))?;
        let (root, progress) = self.physical_profile_begin(
            witness,
            role,
            NativeScanOrder::new(role, &witness.source.path),
        )?;
        if cancelled() {
            return Ok((root, ProfileProgress::Pending, Work::default()));
        }
        let ProfileProgress::Input { ordinal, offset } = progress else {
            if progress == ProfileProgress::Ready {
                return Ok((root, progress, Work::default()));
            }
            let (progress, work) = self.physical_profile_advance(witness, root, cancelled)?;
            return Ok((root, progress, work));
        };
        let mut work = Work::default();
        if ordinal == 0 {
            let native = witness.source.native_id.as_bytes();
            let start = usize::try_from(offset).map_err(io::Error::other)?;
            let end = start.saturating_add(PART).min(native.len());
            let bytes = native
                .get(start..end)
                .ok_or_else(|| io::Error::other("physical own offset changed"))?;
            let progress = self.physical_profile_part(
                witness,
                root,
                ordinal,
                offset,
                AtomKind::Own,
                bytes,
                end == native.len(),
            )?;
            work.metadata_bytes = bytes.len();
            return Ok((root, progress, work));
        }
        let (kind, native) = {
            if role != PhysicalRole::Top {
                self.physical_profile_finish(witness, root, ordinal)?;
                return Ok((root, ProfileProgress::Ready, work));
            }
            let safe:bool=self.connection.query_row("SELECT EXISTS(SELECT 1 FROM native_physical_bootstrap WHERE path=?1 AND wide_links=0)",[witness.source.path.to_str()],|r|r.get(0)).map_err(io::Error::other)?;
            if !safe {
                return Ok((root, ProfileProgress::NativeAtomsRequired, work));
            }
            let saved:Option<(i64,String,String)>=self.connection.query_row("SELECT ordinal,CASE WHEN octet_length(kind)<=16 THEN kind END,CASE WHEN octet_length(native_id)<=4096 THEN native_id END FROM native_physical_current_cursors WHERE root=?1",[root.0.as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(io::Error::other)?;
            if let Some((saved_ordinal, kind, native)) = &saved
                && u64::try_from(*saved_ordinal).ok() == Some(ordinal)
            {
                (kind.clone(), native.clone())
            } else {
                let after = saved
                    .as_ref()
                    .map(|(_, kind, native)| (kind.as_str(), native.as_str()));
                let tx = self
                    .connection
                    .unchecked_transaction()
                    .map_err(io::Error::other)?;
                let file = current_file(&tx, witness).map_err(io::Error::other)?;
                let (after_kind, after_id) = after.unwrap_or(("", ""));
                let mut query=tx.prepare("SELECT CASE WHEN octet_length(kind)<=16 THEN kind END,CASE WHEN octet_length(native_id)<=4096 THEN native_id END FROM native_source_links WHERE file_id=?1 AND (kind,native_id)>(?2,?3) ORDER BY kind,native_id LIMIT 1").map_err(io::Error::other)?;
                let next: Option<(String, String)> = query
                    .query_row(params![file, after_kind, after_id], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .optional()
                    .map_err(io::Error::other)?;
                work.vm_steps = query.get_status(rusqlite::StatementStatus::VmStep);
                drop(query);
                drop(tx);
                let Some((kind, native)) = next else {
                    self.physical_profile_finish(witness, root, ordinal)?;
                    return Ok((root, ProfileProgress::Ready, work));
                };
                work.decoded_rows += 1;
                work.metadata_bytes += kind.len() + native.len();
                self.connection.execute("INSERT INTO native_physical_current_cursors VALUES(?1,?2,?3,?4) ON CONFLICT(root) DO UPDATE SET ordinal=excluded.ordinal,kind=excluded.kind,native_id=excluded.native_id",params![root.0.as_slice(),i64::try_from(ordinal).map_err(io::Error::other)?,kind,native]).map_err(io::Error::other)?;
                (kind, native)
            }
        };
        let start = usize::try_from(offset).map_err(io::Error::other)?;
        let (bytes, last) = if kind == "own" {
            let end = start.saturating_add(PART).min(native.len());
            (
                native
                    .as_bytes()
                    .get(start..end)
                    .ok_or_else(|| io::Error::other("physical input part offset changed"))?
                    .to_vec(),
                end == native.len(),
            )
        } else {
            crate::model::identity_stage::link_atom_part(&kind, &native, start, PART)
                .map_err(io::Error::other)?
        };
        let kind = match kind.as_str() {
            "own" => AtomKind::Own,
            "session" => AtomKind::Session,
            "continued" => AtomKind::Continued,
            "bridge" => AtomKind::Bridge,
            _ => return Err(io::Error::other("invalid physical input kind")),
        };
        let progress =
            self.physical_profile_part(witness, root, ordinal, offset, kind, &bytes, last)?;
        work.metadata_bytes += bytes.len();
        Ok((root, progress, work))
    }
    /// Establishes a producer root and binds this exact physical source proof.
    /// Role/scan descriptor and link revision are independent from event ledgers.
    /// Reusing a completed root after an event-only append does not visit atoms.
    pub(crate) fn physical_profile_begin(
        &self,
        witness: &SourceWitness,
        role: PhysicalRole,
        scan_order: NativeScanOrder,
    ) -> io::Result<(ProfileRoot, ProfileProgress)> {
        if PhysicalRole::recorded(&witness.source) != Some(role)
            || scan_order != NativeScanOrder::new(role, &witness.source.path)
        {
            return Err(io::Error::other(
                "physical source role or native scan descriptor changed",
            ));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = current_file(&tx, witness).map_err(io::Error::other)?;
        let ledger = witness
            .ledger
            .as_ref()
            .ok_or_else(|| io::Error::other("physical Current source is absent"))?;
        let generation = source_fact_generation(ledger).map_err(io::Error::other)?;
        let (first,primitives_ready):(Option<i64>,bool)=tx.query_row("SELECT first_ms,EXISTS(SELECT 1 FROM native_source_facts WHERE file_id=?1 AND generation=?2) FROM files WHERE file_id=?1",params![file,generation],|r|Ok((r.get(0)?,r.get(1)?))).map_err(io::Error::other)?;
        if !primitives_ready {
            return Err(io::Error::other(
                "physical source primitives require bounded repair",
            ));
        }
        let path = witness
            .source
            .path
            .to_str()
            .ok_or_else(|| io::Error::other("nonportable physical source path"))?;
        // Older native publications are rebuilt incrementally; initializing a
        // missing scalar revision does not enumerate their existing links.
        tx.execute(
            "INSERT OR IGNORE INTO native_physical_link_revisions VALUES(?1,1)",
            [path],
        )
        .map_err(io::Error::other)?;
        let revision: i64 = tx
            .query_row(
                "SELECT revision FROM native_physical_link_revisions WHERE path=?1",
                [path],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        let descriptor = serde_json::to_string(&Descriptor {
            native_id: witness.source.native_id.clone(),
            role,
            scan_order,
        })
        .map_err(io::Error::other)?;
        let root = current_root(path, revision, &descriptor)?;
        tx.execute("INSERT OR IGNORE INTO native_physical_profiles(root,path,revision,descriptor,phase) VALUES(?1,?2,?3,?4,'input')",params![root.0.as_slice(),path,revision,descriptor]).map_err(io::Error::other)?;
        ensure_build(&tx, witness, root).map_err(io::Error::other)?;
        tx.execute("INSERT INTO native_physical_bindings VALUES(?1,?2,?3) ON CONFLICT(source) DO UPDATE SET root=excluded.root,first_ms=excluded.first_ms",params![source_key(&witness.source)?.as_slice(),root.0.as_slice(),first]).map_err(io::Error::other)?;
        let progress = progress(&tx, root)?;
        tx.commit().map_err(io::Error::other)?;
        if !self.source_witness_current(witness)? {
            return Err(io::Error::other(
                "physical source changed after profile begin",
            ));
        }
        Ok((root, progress))
    }

    /// Accept one bounded decoded native fragment. The immutable receipt makes
    /// a retry exact even after the native extraction cursor CAS was lost.
    /// No plaintext belongs in that cursor. This transaction does not assemble
    /// canonical parts; the caller drains separately under its next allowance.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn physical_profile_original_fragment(
        &self,
        operation: OriginalProfileOperation<'_, '_>,
        witness: &SourceWitness,
        root: ProfileRoot,
        ordinal: u64,
        offset: u64,
        kind: AtomKind,
        bytes: &[u8],
        last: bool,
    ) -> io::Result<FragmentProgress> {
        if bytes.len() > PART
            || (bytes.is_empty() && !last)
            || (ordinal == 0) != (kind == AtomKind::Own)
        {
            return Err(io::Error::other("invalid physical native fragment"));
        }
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| io::Error::other("physical fragment offset overflow"))?;
        if ordinal == 0 {
            let native = witness.source.native_id.as_bytes();
            let start = usize::try_from(offset).map_err(io::Error::other)?;
            let end = usize::try_from(end).map_err(io::Error::other)?;
            if native.get(start..end) != Some(bytes) || (last && end != native.len()) {
                return Err(io::Error::other(
                    "physical own fragment differs from native ID",
                ));
            }
        }
        if !self.runtime_current(&operation.job.spec)? {
            return Err(io::Error::other("Original physical runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        ensure_build(&tx, witness, root).map_err(io::Error::other)?;
        operation_current(&tx, witness, root, Some(operation), false)?;
        let ordinal_sql = i64::try_from(ordinal).map_err(io::Error::other)?;
        let offset_sql = i64::try_from(offset).map_err(io::Error::other)?;
        let old: Option<(String,Vec<u8>,bool)> = tx.query_row(
            "SELECT CASE WHEN octet_length(kind)<=16 THEN kind END,CASE WHEN octet_length(bytes)<=65536 THEN bytes END,last FROM native_physical_fragments WHERE root=?1 AND ordinal=?2 AND offset=?3",
            params![root.0.as_slice(),ordinal_sql,offset_sql],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))
        ).optional().map_err(io::Error::other)?;
        if let Some((old_kind, old_bytes, old_last)) = old {
            if old_kind != kind.name() || old_bytes != bytes || old_last != last {
                return Err(io::Error::other("physical fragment replay changed"));
            }
        } else {
            let s = state(&tx, root).map_err(io::Error::other)?;
            if s.phase != "input" || s.next != ordinal_sql {
                return Err(io::Error::other("physical fragment atom cursor changed"));
            }
            tx.execute(
                "INSERT OR IGNORE INTO native_physical_carry VALUES(?1,?2,?3,0,0,0,X'',0)",
                params![root.0.as_slice(), ordinal_sql, kind.name()],
            )
            .map_err(io::Error::other)?;
            let valid: bool = tx.query_row("SELECT ordinal=?2 AND kind=?3 AND received=?4 AND consumed=received AND last=0 FROM native_physical_carry WHERE root=?1",params![root.0.as_slice(),ordinal_sql,kind.name(),offset_sql],|r|r.get(0)).map_err(io::Error::other)?;
            if !valid {
                return Err(io::Error::other(
                    "physical fragment requires its previous drain",
                ));
            }
            tx.execute(
                "INSERT INTO native_physical_fragments VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    root.0.as_slice(),
                    ordinal_sql,
                    offset_sql,
                    kind.name(),
                    bytes,
                    last
                ],
            )
            .map_err(io::Error::other)?;
            tx.execute(
                "UPDATE native_physical_carry SET received=?2,last=?3 WHERE root=?1",
                params![
                    root.0.as_slice(),
                    i64::try_from(end).map_err(io::Error::other)?,
                    last
                ],
            )
            .map_err(io::Error::other)?;
        }
        let pending: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM native_physical_carry WHERE root=?1 AND ordinal=?2 AND (consumed<received OR last=1))",params![root.0.as_slice(),ordinal_sql],|r|r.get(0)).map_err(io::Error::other)?;
        let profile = progress(&tx, root)?;
        operation_current(&tx, witness, root, Some(operation), false)?;
        tx.commit().map_err(io::Error::other)?;
        self.physical_operation_after(witness, Some(operation), false)?;
        Ok(FragmentProgress {
            next_offset: end,
            drained: !pending,
            profile,
        })
    }

    /// Consume at most one <=64KiB fragment and publish at most one canonical
    /// part. Carry and receipts live in SQLite; a crash between part publication
    /// and carry CAS replays that exact part through the existing shared kernel.
    pub(crate) fn physical_profile_original_fragment_advance(
        &self,
        operation: OriginalProfileOperation<'_, '_>,
        witness: &SourceWitness,
        root: ProfileRoot,
        ordinal: u64,
    ) -> io::Result<(FragmentProgress, Work)> {
        if !self.runtime_current(&operation.job.spec)? {
            return Err(io::Error::other("Original physical runtime changed"));
        }
        let ordinal_sql = i64::try_from(ordinal).map_err(io::Error::other)?;
        let (received, consumed, offset, mut carry, last, kind) = {
            let tx = self
                .connection
                .unchecked_transaction()
                .map_err(io::Error::other)?;
            ensure_build(&tx, witness, root).map_err(io::Error::other)?;
            operation_current(&tx, witness, root, Some(operation), false)?;
            let value: Option<(i64,i64,i64,Vec<u8>,bool,String)> = tx.query_row("SELECT received,consumed,offset,CASE WHEN octet_length(bytes)<65536 THEN bytes END,last,CASE WHEN octet_length(kind)<=16 THEN kind END FROM native_physical_carry WHERE root=?1 AND ordinal=?2",params![root.0.as_slice(),ordinal_sql],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(io::Error::other)?;
            let Some(value) = value else {
                let profile = progress(&tx, root)?;
                let end: i64=tx.query_row("SELECT offset+octet_length(bytes) FROM native_physical_fragments WHERE root=?1 AND ordinal=?2 ORDER BY offset DESC LIMIT 1",params![root.0.as_slice(),ordinal_sql],|r|r.get(0)).map_err(io::Error::other)?;
                tx.commit().map_err(io::Error::other)?;
                self.physical_operation_after(witness, Some(operation), false)?;
                return Ok((
                    FragmentProgress {
                        next_offset: u64::try_from(end).map_err(io::Error::other)?,
                        drained: true,
                        profile,
                    },
                    Work {
                        decoded_rows: 2,
                        metadata_bytes: 128,
                        vm_steps: 0,
                    },
                ));
            };
            tx.commit().map_err(io::Error::other)?;
            value
        };
        if consumed < 0
            || received < consumed
            || offset < 0
            || offset % PART as i64 != 0
            || offset + carry.len() as i64 != consumed
        {
            return Err(io::Error::other("invalid physical carry cursor"));
        }
        let kind = match kind.as_str() {
            "own" => AtomKind::Own,
            "session" => AtomKind::Session,
            "continued" => AtomKind::Continued,
            "bridge" => AtomKind::Bridge,
            _ => return Err(io::Error::other("invalid physical fragment kind")),
        };
        let mut work = Work {
            decoded_rows: 1,
            metadata_bytes: carry.len() + 128,
            vm_steps: 0,
        };
        let fragment: Vec<u8> = if consumed < received
            || (last && carry.is_empty() && received == 0)
        {
            let bytes:Vec<u8>=self.connection.query_row("SELECT CASE WHEN octet_length(bytes)<=65536 AND last=?4 THEN bytes END FROM native_physical_fragments WHERE root=?1 AND ordinal=?2 AND offset=?3",params![root.0.as_slice(),ordinal_sql,consumed,last],|r|r.get(0)).map_err(io::Error::other)?;
            if consumed.checked_add(bytes.len() as i64) != Some(received) {
                return Err(io::Error::other("physical fragment length changed"));
            }
            work.decoded_rows += 1;
            work.metadata_bytes += bytes.len();
            bytes
        } else {
            Vec::new()
        };
        let take = (PART - carry.len()).min(fragment.len());
        carry.extend_from_slice(&fragment[..take]);
        let remainder = &fragment[take..];
        let publish = carry.len() == PART || last;
        let sealed = publish && last && remainder.is_empty();
        if publish {
            if carry.is_empty() && offset > 0 {
                // The renderer may discover EOF after a full nonfinal part.
                let tx = self
                    .connection
                    .unchecked_transaction()
                    .map_err(io::Error::other)?;
                ensure_build(&tx, witness, root).map_err(io::Error::other)?;
                operation_current(&tx, witness, root, Some(operation), false)?;
                let s = state(&tx, root).map_err(io::Error::other)?;
                let atom:i64=tx.query_row("SELECT atom FROM native_physical_atoms WHERE root=?1 AND ordinal=?2 AND kind=?3 AND length=?4 AND length%65536=0",params![root.0.as_slice(),ordinal_sql,kind.name(),offset],|r|r.get(0)).map_err(io::Error::other)?;
                let (_, _, complete) = atom_info(&tx, atom).map_err(io::Error::other)?;
                if !complete {
                    if s.phase != "input" || s.atom != Some(atom) {
                        return Err(io::Error::other(
                            "physical terminal fragment cursor changed",
                        ));
                    }
                    tx.execute(
                        "UPDATE native_physical_atoms SET complete=1 WHERE atom=?1",
                        [atom],
                    )
                    .map_err(io::Error::other)?;
                    tx.execute("UPDATE native_physical_profiles SET phase='minimum',compare_atom=minimum,compare_part=0,peer_after=0,continued=continued+?2 WHERE root=?1",params![root.0.as_slice(),kind==AtomKind::Continued]).map_err(io::Error::other)?;
                }
                operation_current(&tx, witness, root, Some(operation), false)?;
                tx.commit().map_err(io::Error::other)?;
                self.physical_operation_after(witness, Some(operation), false)?;
            } else {
                self.physical_profile_original_part(
                    operation,
                    witness,
                    root,
                    ordinal,
                    u64::try_from(offset).map_err(io::Error::other)?,
                    kind,
                    &carry,
                    sealed,
                )?;
            }
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        ensure_build(&tx, witness, root).map_err(io::Error::other)?;
        operation_current(&tx, witness, root, Some(operation), false)?;
        let changed = if sealed {
            tx.execute("DELETE FROM native_physical_carry WHERE root=?1 AND ordinal=?2 AND consumed=?3 AND received=?4 AND offset=?5",params![root.0.as_slice(),ordinal_sql,consumed,received,offset]).map_err(io::Error::other)?
        } else {
            let (next_offset, bytes) = if publish {
                (offset + PART as i64, remainder)
            } else {
                (offset, carry.as_slice())
            };
            tx.execute("UPDATE native_physical_carry SET consumed=received,offset=?6,bytes=?7 WHERE root=?1 AND ordinal=?2 AND consumed=?3 AND received=?4 AND offset=?5",params![root.0.as_slice(),ordinal_sql,consumed,received,offset,next_offset,bytes]).map_err(io::Error::other)?
        };
        if changed != 1 {
            return Err(io::Error::other("physical carry CAS changed"));
        }
        let profile = progress(&tx, root)?;
        operation_current(&tx, witness, root, Some(operation), false)?;
        tx.commit().map_err(io::Error::other)?;
        self.physical_operation_after(witness, Some(operation), false)?;
        Ok((
            FragmentProgress {
                next_offset: u64::try_from(received).map_err(io::Error::other)?,
                drained: sealed || !last,
                profile,
            },
            work,
        ))
    }

    /// One CAS input part. The native owner emits raw atoms using star_atoms /
    /// link_atom, including the bridge prefix, and checkpoints its extraction
    /// cursor with the returned ordinal/offset. No complete atom is retained.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn physical_profile_part(
        &self,
        witness: &SourceWitness,
        root: ProfileRoot,
        ordinal: u64,
        offset: u64,
        kind: AtomKind,
        bytes: &[u8],
        last: bool,
    ) -> io::Result<ProfileProgress> {
        self.physical_profile_part_inner(None, witness, root, ordinal, offset, kind, bytes, last)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn physical_profile_original_part(
        &self,
        operation: OriginalProfileOperation<'_, '_>,
        witness: &SourceWitness,
        root: ProfileRoot,
        ordinal: u64,
        offset: u64,
        kind: AtomKind,
        bytes: &[u8],
        last: bool,
    ) -> io::Result<ProfileProgress> {
        self.physical_profile_part_inner(
            Some(operation),
            witness,
            root,
            ordinal,
            offset,
            kind,
            bytes,
            last,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn physical_profile_part_inner(
        &self,
        operation: Option<OriginalProfileOperation<'_, '_>>,
        witness: &SourceWitness,
        root: ProfileRoot,
        ordinal: u64,
        offset: u64,
        kind: AtomKind,
        bytes: &[u8],
        last: bool,
    ) -> io::Result<ProfileProgress> {
        if let Some(operation) = operation
            && !self.runtime_current(&operation.job.spec)?
        {
            return Err(io::Error::other("Original physical runtime changed"));
        }
        if bytes.len() > PART
            || (!last && bytes.len() != PART)
            || (bytes.is_empty() && offset != 0)
            || offset % PART as u64 != 0
        {
            return Err(io::Error::other("physical atom part is not canonical"));
        }
        if (ordinal == 0) != (kind == AtomKind::Own) {
            return Err(io::Error::other(
                "physical star must begin with its native own ID",
            ));
        }
        if ordinal == 0 {
            let native = witness.source.native_id.as_bytes();
            let start = usize::try_from(offset).map_err(io::Error::other)?;
            let end = start
                .checked_add(bytes.len())
                .ok_or_else(|| io::Error::other("physical own atom offset overflow"))?;
            if native.get(start..end) != Some(bytes) || (last && end != native.len()) {
                return Err(io::Error::other("physical own atom differs from native ID"));
            }
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        ensure_build(&tx, witness, root).map_err(io::Error::other)?;
        operation_current(&tx, witness, root, operation, false)?;
        let s = state(&tx, root).map_err(io::Error::other)?;
        let existing:Option<(i64,String)>=tx.query_row("SELECT atom,CASE WHEN octet_length(kind)<=16 THEN kind END FROM native_physical_atoms WHERE root=?1 AND ordinal=?2",params![root.0.as_slice(),i64::try_from(ordinal).map_err(io::Error::other)?],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(io::Error::other)?;
        if let Some((atom, stored_kind)) = existing {
            let (length, _, complete) = atom_info(&tx, atom).map_err(io::Error::other)?;
            if offset < u64::try_from(length).map_err(io::Error::other)?
                || (length == 0 && complete)
            {
                let stored = part(
                    &tx,
                    atom,
                    i64::try_from(offset / PART as u64).map_err(io::Error::other)?,
                    &mut Work::default(),
                )?;
                let expected_last = complete
                    && offset.checked_add(bytes.len() as u64) == u64::try_from(length).ok();
                if stored_kind != kind.name() || stored != bytes || expected_last != last {
                    return Err(io::Error::other(
                        "physical atom replay differs from committed part",
                    ));
                }
                operation_current(&tx, witness, root, operation, false)?;
                let progress = progress(&tx, root)?;
                tx.commit().map_err(io::Error::other)?;
                self.physical_operation_after(witness, operation, false)?;
                if let Some(operation) = operation
                    && !self.runtime_current(&operation.job.spec)?
                {
                    return Err(io::Error::other(
                        "Original physical runtime changed after replay",
                    ));
                }
                if !self.source_witness_current(witness)? {
                    return Err(io::Error::other("physical source changed after replay"));
                }
                return Ok(progress);
            }
        }
        if s.phase != "input" || u64::try_from(s.next).ok() != Some(ordinal) {
            return Err(io::Error::other("physical input cursor changed"));
        }
        let atom = match s.atom {
            Some(atom) => atom,
            None => {
                if offset != 0 {
                    return Err(io::Error::other("physical atom part is missing its prefix"));
                }
                tx.execute("INSERT INTO native_physical_atoms(root,ordinal,kind,digest) VALUES(?1,?2,?3,?4)",params![root.0.as_slice(),s.next,kind.name(),[0u8;32].as_slice()]).map_err(io::Error::other)?;
                let atom = tx.last_insert_rowid();
                tx.execute(
                    "UPDATE native_physical_profiles SET atom=?2 WHERE root=?1",
                    params![root.0.as_slice(), atom],
                )
                .map_err(io::Error::other)?;
                atom
            }
        };
        let (length, previous, complete) = atom_info(&tx, atom).map_err(io::Error::other)?;
        let actual_kind: String = tx
            .query_row(
                "SELECT kind FROM native_physical_atoms WHERE atom=?1",
                [atom],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        if u64::try_from(length).ok() != Some(offset) || complete || actual_kind != kind.name() {
            return Err(io::Error::other("physical atom offset or kind changed"));
        }
        let mut hash = Sha256::new();
        hash.update(previous);
        hash.update(bytes);
        let digest: [u8; 32] = hash.finalize().into();
        tx.execute(
            "INSERT INTO native_physical_atom_parts VALUES(?1,?2,?3)",
            params![atom, length / PART as i64, bytes],
        )
        .map_err(io::Error::other)?;
        tx.execute(
            "INSERT INTO native_physical_part_hashes VALUES(?1,?2,?3)",
            params![atom, length / PART as i64, digest.as_slice()],
        )
        .map_err(io::Error::other)?;
        tx.execute(
            "UPDATE native_physical_atoms SET length=length+?2,digest=?3,complete=?4 WHERE atom=?1",
            params![atom, bytes.len() as i64, digest.as_slice(), last],
        )
        .map_err(io::Error::other)?;
        if last {
            tx.execute("UPDATE native_physical_profiles SET phase='minimum',compare_atom=minimum,compare_part=0,peer_after=0,continued=continued+?2 WHERE root=?1",params![root.0.as_slice(),kind==AtomKind::Continued]).map_err(io::Error::other)?;
        }
        let progress = progress(&tx, root)?;
        operation_current(&tx, witness, root, operation, false)?;
        tx.commit().map_err(io::Error::other)?;
        self.physical_operation_after(witness, operation, false)?;
        if let Some(operation) = operation
            && !self.runtime_current(&operation.job.spec)?
        {
            return Err(io::Error::other(
                "Original physical runtime changed after atom part",
            ));
        }
        if !self.source_witness_current(witness)? {
            return Err(io::Error::other("physical source changed after atom part"));
        }
        Ok(progress)
    }

    /// One bounded comparison/probe checkpoint; high fanout is durable work,
    /// rather than an unbounded INSERT SELECT hidden behind a small row count.
    pub(crate) fn physical_profile_advance(
        &self,
        witness: &SourceWitness,
        root: ProfileRoot,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<(ProfileProgress, Work)> {
        self.physical_profile_advance_inner(None, witness, root, cancelled)
    }
    pub(crate) fn physical_profile_original_advance(
        &self,
        operation: OriginalProfileOperation<'_, '_>,
        witness: &SourceWitness,
        root: ProfileRoot,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<(ProfileProgress, Work)> {
        self.physical_profile_advance_inner(Some(operation), witness, root, cancelled)
    }
    fn physical_profile_advance_inner(
        &self,
        operation: Option<OriginalProfileOperation<'_, '_>>,
        witness: &SourceWitness,
        root: ProfileRoot,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<(ProfileProgress, Work)> {
        if let Some(operation) = operation
            && !self.runtime_current(&operation.job.spec)?
        {
            return Err(io::Error::other("Original physical runtime changed"));
        }
        let mut work = Work::default();
        if cancelled() {
            return Ok((ProfileProgress::Pending, work));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        ensure_build(&tx, witness, root).map_err(io::Error::other)?;
        operation_current(&tx, witness, root, operation, false)?;
        let s = state(&tx, root).map_err(io::Error::other)?;
        match s.phase.as_str() {
            "minimum" => {
                let atom = s
                    .atom
                    .ok_or_else(|| io::Error::other("physical minimum lacks atom"))?;
                let comparison = match s.compare {
                    Some(other) => compare_part(&tx, atom, other, s.part, &mut work)?,
                    None => Some(Ordering::Less),
                };
                if let Some(order) = comparison {
                    tx.execute("UPDATE native_physical_profiles SET minimum=CASE WHEN ?2 THEN atom ELSE minimum END,phase='peers',compare_atom=NULL,compare_part=0 WHERE root=?1",params![root.0.as_slice(),order.is_lt()]).map_err(io::Error::other)?;
                } else {
                    tx.execute("UPDATE native_physical_profiles SET compare_part=compare_part+1 WHERE root=?1",[root.0.as_slice()]).map_err(io::Error::other)?;
                }
            }
            "peers" => {
                let atom = s
                    .atom
                    .ok_or_else(|| io::Error::other("physical peer scan lacks atom"))?;
                let peer=match s.compare {
                    Some(peer)=>Some(peer),
                    None=>tx.query_row("SELECT b.atom FROM native_physical_atoms a JOIN native_physical_atoms b ON b.digest=a.digest AND b.length=a.length AND b.complete=1 WHERE a.atom=?1 AND b.atom>?2 AND b.root<>a.root ORDER BY b.atom LIMIT 1",params![atom,s.peer_after],|r|r.get(0)).optional().map_err(io::Error::other)?,
                };
                if let Some(peer) = peer {
                    match compare_part(&tx, atom, peer, s.part, &mut work)? {
                        None => {
                            tx.execute("UPDATE native_physical_profiles SET compare_atom=?2,compare_part=compare_part+1 WHERE root=?1",params![root.0.as_slice(),peer]).map_err(io::Error::other)?;
                        }
                        Some(order) => {
                            if order.is_eq() {
                                tx.execute("INSERT OR IGNORE INTO native_physical_pairs SELECT min(?1,root),max(?1,root) FROM native_physical_atoms WHERE atom=?2",params![root.0.as_slice(),peer]).map_err(io::Error::other)?;
                            }
                            tx.execute("UPDATE native_physical_profiles SET peer_after=?2,compare_atom=NULL,compare_part=0 WHERE root=?1",params![root.0.as_slice(),peer]).map_err(io::Error::other)?;
                        }
                    }
                } else {
                    tx.execute("UPDATE native_physical_profiles SET phase='input',next_atom=next_atom+1,atom=NULL,compare_atom=NULL,compare_part=0 WHERE root=?1",[root.0.as_slice()]).map_err(io::Error::other)?;
                }
            }
            "input" | "ready" => {}
            _ => return Err(io::Error::other("invalid physical profile phase")),
        }
        if cancelled() {
            return Ok((ProfileProgress::Pending, work));
        }
        ensure_build(&tx, witness, root).map_err(io::Error::other)?;
        let progress = progress(&tx, root)?;
        operation_current(&tx, witness, root, operation, false)?;
        tx.commit().map_err(io::Error::other)?;
        self.physical_operation_after(witness, operation, false)?;
        if let Some(operation) = operation
            && !self.runtime_current(&operation.job.spec)?
        {
            return Err(io::Error::other(
                "Original physical runtime changed after advance",
            ));
        }
        if !self.source_witness_current(witness)? {
            return Err(io::Error::other(
                "physical source changed after profile advance",
            ));
        }
        Ok((progress, work))
    }

    /// The native owner's completed extraction cursor is required. A prefix
    /// of staged atoms never becomes a ready profile merely because it is idle.
    pub(crate) fn physical_profile_finish(
        &self,
        witness: &SourceWitness,
        root: ProfileRoot,
        atoms: u64,
    ) -> io::Result<()> {
        self.physical_profile_finish_inner(None, witness, root, atoms)
    }
    pub(crate) fn physical_profile_original_finish(
        &self,
        operation: OriginalProfileOperation<'_, '_>,
        witness: &SourceWitness,
        root: ProfileRoot,
        atoms: u64,
    ) -> io::Result<()> {
        self.physical_profile_finish_inner(Some(operation), witness, root, atoms)
    }
    fn physical_profile_finish_inner(
        &self,
        operation: Option<OriginalProfileOperation<'_, '_>>,
        witness: &SourceWitness,
        root: ProfileRoot,
        atoms: u64,
    ) -> io::Result<()> {
        if let Some(operation) = operation
            && !self.runtime_current(&operation.job.spec)?
        {
            return Err(io::Error::other("Original physical runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        ensure_build(&tx, witness, root).map_err(io::Error::other)?;
        operation_current(&tx, witness, root, operation, true)?;
        let s = state(&tx, root).map_err(io::Error::other)?;
        if s.phase == "ready"
            && u64::try_from(s.next).ok() == Some(atoms)
            && complete_counts(&tx, root, s.next).map_err(io::Error::other)?
        {
            tx.commit().map_err(io::Error::other)?;
            self.physical_operation_after(witness, operation, true)?;
            return Ok(());
        }
        if s.phase != "input"
            || s.atom.is_some()
            || atoms == 0
            || u64::try_from(s.next).ok() != Some(atoms)
            || s.minimum.is_none()
            || !complete_counts(&tx, root, s.next).map_err(io::Error::other)?
        {
            return Err(io::Error::other(
                "physical profile extraction is incomplete",
            ));
        }
        tx.execute(
            "UPDATE native_physical_profiles SET phase='ready' WHERE root=?1",
            [root.0.as_slice()],
        )
        .map_err(io::Error::other)?;
        tx.commit().map_err(io::Error::other)?;
        self.physical_operation_after(witness, operation, true)?;
        if let Some(operation) = operation
            && !self.runtime_current(&operation.job.spec)?
        {
            return Err(io::Error::other(
                "Original physical runtime changed after finish",
            ));
        }
        if !self.source_witness_current(witness)? {
            return Err(io::Error::other(
                "physical source changed after profile finish",
            ));
        }
        Ok(())
    }

    /// Only completed, congruent scalar observations are admitted by readers.
    fn physical_profile_base(&self, witness: &SourceWitness) -> io::Result<PhysicalSource> {
        if !self.source_witness_current(witness)? {
            return Err(io::Error::other("physical profile witness changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let (root,first,descriptor,continued,revision,original,next):(Vec<u8>,Option<i64>,String,i64,i64,Option<Vec<u8>>,i64)=tx.query_row("SELECT CASE WHEN octet_length(p.root)=32 THEN p.root END,b.first_ms,CASE WHEN octet_length(p.descriptor)<=?3 THEN p.descriptor END,p.continued,p.revision,CASE WHEN p.original IS NULL OR octet_length(p.original)=32 THEN p.original ELSE zeroblob(0) END,p.next_atom FROM native_physical_bindings b JOIN native_physical_profiles p ON p.root=b.root WHERE b.source=?1 AND p.path=?2 AND p.phase='ready' AND p.continued>=0",params![source_key(&witness.source)?.as_slice(),witness.source.path.to_str(),i64::try_from(descriptor_bound(witness)).map_err(io::Error::other)?],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).map_err(io::Error::other)?;
        let root = ProfileRoot(
            root.try_into()
                .map_err(|_| io::Error::other("invalid physical root"))?,
        );
        let expected_root = if let Some(original) = original {
            let source = source_key(&witness.source)?;
            if original != source {
                return Err(io::Error::other("physical Original binding differs"));
            }
            ProfileRoot(digest(
                &serde_json::to_vec(&("physical-original-v2", source, &descriptor))
                    .map_err(io::Error::other)?,
            ))
        } else {
            current_root(
                witness
                    .source
                    .path
                    .to_str()
                    .ok_or_else(|| io::Error::other("invalid physical path"))?,
                revision,
                &descriptor,
            )?
        };
        let descriptor: Descriptor = serde_json::from_str(&descriptor).map_err(io::Error::other)?;
        if root != expected_root
            || descriptor.native_id != witness.source.native_id
            || descriptor.scan_order != NativeScanOrder::new(descriptor.role, &witness.source.path)
            || PhysicalRole::recorded(&witness.source).is_some_and(|role| role != descriptor.role)
            || !complete_counts(&tx, root, next).map_err(io::Error::other)?
        {
            return Err(io::Error::other(
                "physical profile descriptor or completion proof changed",
            ));
        }
        drop(tx);
        if !self.source_witness_current(witness)? {
            return Err(io::Error::other(
                "physical witness changed after scalar read",
            ));
        }
        Ok(PhysicalSource {
            profile: root,
            native_id: descriptor.native_id,
            role: descriptor.role,
            scan_order: descriptor.scan_order,
            first,
            continued_empty: continued == 0,
            turns_truncated: false,
        })
    }

    pub(crate) fn physical_profile_for_context(
        &self,
        witness: &SourceWitness,
        context: [u8; 32],
    ) -> io::Result<PhysicalSource> {
        let mut source = self.physical_profile_base(witness)?;
        let answer: Option<bool> = self
            .connection
            .query_row(
                "SELECT CASE WHEN answer IN (0,1) THEN answer END FROM native_physical_truncations WHERE root=?1 AND context=?2",
                params![source.profile.0.as_slice(), context.as_slice()],
                |r| r.get(0),
            )
            .optional()
            .map_err(io::Error::other)?
            .flatten();
        source.turns_truncated = answer.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "physical skipped context requires bounded preparation",
            )
        })?;
        if !self.source_witness_current(witness)? {
            return Err(io::Error::other(
                "physical source changed after context read",
            ));
        }
        Ok(source)
    }

    /// Finds already qualified pure evidence for a new omitted-source context,
    /// including retained Original whose Current ledger is absent or newer.
    /// It supplies no truncation result or owner/custody authority.
    pub(crate) fn physical_profile_root(&self, witness: &SourceWitness) -> io::Result<ProfileRoot> {
        Ok(self.physical_profile_base(witness)?.profile)
    }

    /// Native closure owns the skipped filename set and its context digest.
    /// At most 64 bounded IDs are processed; all work and the source ordinal
    /// commit together. A complete false answer requires its final native page.
    pub(crate) fn physical_truncation_step(
        &self,
        root: ProfileRoot,
        context: [u8; 32],
        after: u64,
        skipped: &[String],
        final_page: bool,
    ) -> io::Result<(TruncationProgress, Work)> {
        if skipped.len() > ROWS
            || skipped.iter().map(String::len).sum::<usize>() > STATE_BYTES
            || skipped.iter().any(|id| id.len() > KEY_BYTES)
        {
            return Err(io::Error::other(
                "physical skipped filename page exceeds native path budget",
            ));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let s = state(&tx, root).map_err(io::Error::other)?;
        if s.phase != "ready" || !complete_counts(&tx, root, s.next).map_err(io::Error::other)? {
            return Err(io::Error::other("physical truncation root is incomplete"));
        }
        tx.execute(
            "INSERT OR IGNORE INTO native_physical_truncations(root,context,next) VALUES(?1,?2,0)",
            params![root.0.as_slice(), context.as_slice()],
        )
        .map_err(io::Error::other)?;
        let (next,answer,mut candidate):(i64,Option<bool>,i64)=tx.query_row("SELECT next,answer,candidate FROM native_physical_truncations WHERE root=?1 AND context=?2",params![root.0.as_slice(),context.as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(io::Error::other)?;
        if answer.is_some() {
            return Ok((
                TruncationProgress {
                    next: u64::try_from(next).map_err(io::Error::other)?,
                    answer,
                },
                Work::default(),
            ));
        }
        if u64::try_from(next).ok() != Some(after) {
            return Err(io::Error::other("physical skipped page cursor changed"));
        }
        let mut work = Work::default();
        let mut answer = None;
        let mut processed = 0usize;
        // Reserve rows for atom/header/hash and durable controller observations,
        // rather than spending the whole page allowance on bucket candidates.
        let mut remaining = ROWS / 8;
        for id in skipped {
            if remaining == 0 || processed == ROWS / 8 {
                break;
            }
            work.decoded_rows += 1;
            work.metadata_bytes += id.len();
            let mut hash = Sha256::new();
            hash.update([0u8; 32]);
            hash.update(id.as_bytes());
            let chain: [u8; 32] = hash.finalize().into();
            let mut q=tx.prepare("SELECT atom FROM native_physical_atoms WHERE root=?1 AND kind='session' AND digest=?2 AND length=?3 AND complete=1 AND atom>?4 ORDER BY atom LIMIT ?5").map_err(io::Error::other)?;
            let atoms = q
                .query_map(
                    params![
                        root.0.as_slice(),
                        chain.as_slice(),
                        sequence_sql(id.len()).map_err(io::Error::other)?,
                        candidate,
                        (remaining + 1) as i64
                    ],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(io::Error::other)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(io::Error::other)?;
            work.vm_steps += q.get_status(rusqlite::StatementStatus::VmStep);
            work.decoded_rows += atoms.len();
            work.metadata_bytes += atoms.len() * 8;
            let complete = atoms.len() <= remaining;
            for atom in atoms.into_iter().take(remaining) {
                candidate = atom;
                remaining -= 1;
                if part(&tx, atom, 0, &mut work)? == id.as_bytes() {
                    answer = Some(true);
                    break;
                }
            }
            if answer.is_some() {
                processed += 1;
                break;
            }
            if !complete {
                break;
            }
            processed += 1;
            candidate = 0;
        }
        if answer.is_none() && final_page && processed == skipped.len() {
            answer = Some(false);
        }
        let next = after
            .checked_add(processed as u64)
            .and_then(|v| i64::try_from(v).ok())
            .ok_or_else(|| io::Error::other("physical skipped ordinal overflow"))?;
        tx.execute("UPDATE native_physical_truncations SET next=?3,answer=?4,candidate=?5 WHERE root=?1 AND context=?2",params![root.0.as_slice(),context.as_slice(),next,answer,candidate]).map_err(io::Error::other)?;
        tx.commit().map_err(io::Error::other)?;
        Ok((
            TruncationProgress {
                next: u64::try_from(next).map_err(io::Error::other)?,
                answer,
            },
            work,
        ))
    }

    #[cfg(test)]
    fn physical_profile(
        &self,
        witness: &SourceWitness,
        skipped: &[String],
    ) -> io::Result<PhysicalSource> {
        let source = self.physical_profile_base(witness)?;
        let context = digest(&serde_json::to_vec(skipped).map_err(io::Error::other)?);
        let mut next = 0usize;
        loop {
            let end = next.saturating_add(ROWS).min(skipped.len());
            let (progress, _) = self.physical_truncation_step(
                source.profile,
                context,
                next as u64,
                &skipped[next..end],
                end == skipped.len(),
            )?;
            if progress.answer.is_some() {
                break;
            }
            next = usize::try_from(progress.next).map_err(io::Error::other)?;
        }
        self.physical_profile_for_context(witness, context)
    }

    pub(crate) fn physical_pair(&self, left: ProfileRoot, right: ProfileRoot) -> io::Result<bool> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        for root in [left, right] {
            ready_state(&tx, root)?;
        }
        if left == right {
            return Ok(true);
        }
        let (left, right) = if left < right {
            (left, right)
        } else {
            (right, left)
        };
        tx.query_row("SELECT EXISTS(SELECT 1 FROM native_physical_pairs WHERE left_root=?1 AND right_root=?2)",params![left.0.as_slice(),right.0.as_slice()],|r|r.get(0)).map_err(io::Error::other)
    }

    /// Sparse adjacency is enumerated by the two pair indexes. The native
    /// selected-context producer filters eligible roots and checkpoints pages;
    /// neither missing physical connectors nor unrelated roots enter a union.
    pub(crate) fn physical_neighbors(
        &self,
        root: ProfileRoot,
        after: Option<ProfileRoot>,
    ) -> io::Result<Page<ProfileRoot, ProfileRoot>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let s = state(&tx, root).map_err(io::Error::other)?;
        if s.phase != "ready" || !complete_counts(&tx, root, s.next).map_err(io::Error::other)? {
            return Err(io::Error::other("physical adjacency root is incomplete"));
        }
        let after_bytes = after.as_ref().map(|root| root.0.as_slice()).unwrap_or(&[]);
        let mut q=tx.prepare("SELECT CASE WHEN octet_length(e.right_root)=32 THEN e.right_root END AS neighbor,e.right_root AS order_root FROM native_physical_pairs e JOIN native_physical_profiles p ON p.root=e.right_root AND p.phase='ready' WHERE e.left_root=?1 AND e.right_root>?2 UNION ALL SELECT CASE WHEN octet_length(e.left_root)=32 THEN e.left_root END AS neighbor,e.left_root AS order_root FROM native_physical_pairs e JOIN native_physical_profiles p ON p.root=e.left_root AND p.phase='ready' WHERE e.right_root=?1 AND e.left_root>?2 ORDER BY order_root LIMIT 16").map_err(io::Error::other)?;
        let mut rows = q
            .query(params![root.0.as_slice(), after_bytes])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes: Vec<u8> = row.get(0).map_err(io::Error::other)?;
            output.push(ProfileRoot(
                bytes
                    .try_into()
                    .map_err(|_| io::Error::other("invalid physical adjacency root"))?,
            ));
            work.decoded_rows += 1;
            work.metadata_bytes += 32;
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        let complete = output.len() < ROWS / 4;
        let after = output.last().copied().or(after);
        Ok(Page {
            rows: output,
            after,
            complete,
            work,
        })
    }

    /// A single comparison part for opaque raw minima. A native producer can
    /// checkpoint `part` until Some(order); no full raw atom is decoded.
    pub(crate) fn physical_minimum_order_part(
        &self,
        left: ProfileRoot,
        right: ProfileRoot,
        index: u64,
    ) -> io::Result<(Option<Ordering>, Work)> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let (left, right) = (ready_state(&tx, left)?, ready_state(&tx, right)?);
        if left.phase != "ready" || right.phase != "ready" {
            return Err(io::Error::other("physical minimum is incomplete"));
        }
        let mut work = Work::default();
        let order = compare_part(
            &tx,
            left.minimum
                .ok_or_else(|| io::Error::other("missing physical minimum"))?,
            right
                .minimum
                .ok_or_else(|| io::Error::other("missing physical minimum"))?,
            i64::try_from(index).map_err(io::Error::other)?,
            &mut work,
        )?;
        Ok((order, work))
    }

    /// Background preparation for one selected-context minimum comparison.
    /// Its continuation is durable and independent of the requested page.
    pub(crate) fn physical_minimum_order_advance(
        &self,
        left: ProfileRoot,
        right: ProfileRoot,
    ) -> io::Result<(Option<Ordering>, Work)> {
        if left == right {
            return Ok((Some(Ordering::Equal), Work::default()));
        }
        let (low, high, reversed) = if left < right {
            (left, right, false)
        } else {
            (right, left, true)
        };
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        tx.execute(
            "INSERT OR IGNORE INTO native_physical_orders(left_root,right_root) VALUES(?1,?2)",
            params![low.0.as_slice(), high.0.as_slice()],
        )
        .map_err(io::Error::other)?;
        let (index,ordering):(i64,Option<i64>)=tx.query_row("SELECT part,ordering FROM native_physical_orders WHERE left_root=?1 AND right_root=?2",params![low.0.as_slice(),high.0.as_slice()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(io::Error::other)?;
        ready_state(&tx, low)?;
        ready_state(&tx, high)?;
        if let Some(ordering) = ordering {
            return Ok((
                Some(if reversed {
                    ordering.cmp(&0).reverse()
                } else {
                    ordering.cmp(&0)
                }),
                Work::default(),
            ));
        }
        // No nested SQLite snapshot is needed; both immutable roots are read
        // from this transaction and their source fences belong to the caller.
        let a = ready_state(&tx, low)?;
        let b = ready_state(&tx, high)?;
        if a.phase != "ready" || b.phase != "ready" {
            return Err(io::Error::other("physical minimum roots are incomplete"));
        }
        let mut work = Work::default();
        let order = compare_part(
            &tx,
            a.minimum
                .ok_or_else(|| io::Error::other("missing physical minimum"))?,
            b.minimum
                .ok_or_else(|| io::Error::other("missing physical minimum"))?,
            index,
            &mut work,
        )?;
        let numeric = order.map(|order| match order {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        });
        tx.execute("UPDATE native_physical_orders SET part=part+1,ordering=?3 WHERE left_root=?1 AND right_root=?2",params![low.0.as_slice(),high.0.as_slice(),numeric]).map_err(io::Error::other)?;
        tx.commit().map_err(io::Error::other)?;
        Ok((
            order.map(|order| if reversed { order.reverse() } else { order }),
            work,
        ))
    }

    /// Foreground selection never drains a long common raw-atom prefix.
    pub(crate) fn physical_minimum_order(
        &self,
        left: ProfileRoot,
        right: ProfileRoot,
    ) -> io::Result<Ordering> {
        if left == right {
            return Ok(Ordering::Equal);
        }
        let (low, high, reversed) = if left < right {
            (left, right, false)
        } else {
            (right, left, true)
        };
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        ready_state(&tx, low)?;
        ready_state(&tx, high)?;
        let order: Option<i64> = tx
            .query_row(
                "SELECT ordering FROM native_physical_orders WHERE left_root=?1 AND right_root=?2 AND ordering IN (-1,0,1)",
                params![low.0.as_slice(), high.0.as_slice()],
                |r| r.get(0),
            )
            .optional()
            .map_err(io::Error::other)?
            .flatten();
        let order = order
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "physical minimum comparison requires bounded preparation",
                )
            })?
            .cmp(&0);
        Ok(if reversed { order.reverse() } else { order })
    }
}
