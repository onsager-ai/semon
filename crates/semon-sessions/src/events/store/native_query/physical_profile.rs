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
pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_physical_link_revisions (
 path TEXT PRIMARY KEY, revision INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_physical_profiles (
 root BLOB PRIMARY KEY, path TEXT NOT NULL, revision INTEGER NOT NULL,
 descriptor TEXT NOT NULL, phase TEXT NOT NULL, original BLOB,
 next_atom INTEGER NOT NULL DEFAULT 0, atom INTEGER,
 minimum INTEGER, compare_atom INTEGER, compare_part INTEGER NOT NULL DEFAULT 0,
 peer_after INTEGER NOT NULL DEFAULT 0, continued INTEGER NOT NULL DEFAULT 0
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_physical_profile_path ON native_physical_profiles(path,revision);
CREATE TABLE IF NOT EXISTS native_physical_atoms (
 atom INTEGER PRIMARY KEY, root BLOB NOT NULL, ordinal INTEGER NOT NULL,
 kind TEXT NOT NULL, length INTEGER NOT NULL DEFAULT 0,
 digest BLOB NOT NULL, complete INTEGER NOT NULL DEFAULT 0,
 UNIQUE(root,ordinal)
) STRICT;
CREATE INDEX IF NOT EXISTS native_physical_atom_bucket ON native_physical_atoms(digest,length,atom) WHERE complete=1;
CREATE TABLE IF NOT EXISTS native_physical_atom_parts (
 atom INTEGER NOT NULL, part INTEGER NOT NULL, bytes BLOB NOT NULL,
 PRIMARY KEY(atom,part), CHECK(length(bytes)<=65536)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_physical_pairs (
 left_root BLOB NOT NULL, right_root BLOB NOT NULL,
 PRIMARY KEY(left_root,right_root), CHECK(left_root<right_root)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_physical_bindings (
 source BLOB PRIMARY KEY, root BLOB NOT NULL, first_ms INTEGER
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_physical_session_bucket ON native_physical_atoms(root,kind,digest,length,atom) WHERE complete=1;
CREATE TABLE IF NOT EXISTS native_physical_orders (
 left_root BLOB NOT NULL, right_root BLOB NOT NULL,
 part INTEGER NOT NULL DEFAULT 0, ordering INTEGER,
 PRIMARY KEY(left_root,right_root), CHECK(left_root<right_root)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_physical_current_cursors (
 root BLOB PRIMARY KEY, ordinal INTEGER NOT NULL, kind TEXT NOT NULL, native_id TEXT NOT NULL
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
            let (root, progress) = fixture
                .store
                .physical_profile_begin_original(
                    &job,
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
                .physical_profile_part(&witness, root, 0, 0, AtomKind::Own, b"original", true)
                .unwrap();
            while fixture
                .store
                .physical_profile_advance(&witness, root, &|| false)
                .unwrap()
                .0
                == ProfileProgress::Pending
            {}
            fixture
                .store
                .physical_profile_part(&witness, root, 1, 0, AtomKind::Session, b"skipped", true)
                .unwrap();
            while fixture
                .store
                .physical_profile_advance(&witness, root, &|| false)
                .unwrap()
                .0
                == ProfileProgress::Pending
            {}
            fixture
                .store
                .physical_profile_finish(&witness, root, 2)
                .unwrap();
            let profile = fixture
                .store
                .physical_profile(&witness, &["skipped".into()])
                .unwrap();
            assert_eq!(profile.first, Some(-9));
            assert!(profile.turns_truncated);
            // Observation changes invalidate every feed entry point even
            // though the immutable Original source proof remains unchanged.
            fixture.store.connection.execute("INSERT INTO meta(key,value) VALUES(?1,'changed') ON CONFLICT(key) DO UPDATE SET value=excluded.value",[archive_index_key(&source.path.to_string_lossy())]).unwrap();
            assert!(
                fixture
                    .store
                    .physical_profile_advance(&witness, root, &|| false)
                    .is_err()
            );
            assert!(
                fixture
                    .store
                    .physical_profile_finish(&witness, root, 2)
                    .is_err()
            );
            assert!(fixture.store.physical_profile(&witness, &[]).is_err());
        }
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
        db.execute("INSERT INTO native_physical_atoms VALUES(1,X'01',0,'own',1,X'00',1),(2,X'02',0,'own',1,X'00',1)",[]).unwrap();
        db.execute(
            "INSERT INTO native_physical_atom_parts VALUES(1,0,X'61'),(2,0,X'62')",
            [],
        )
        .unwrap();
        assert_eq!(
            compare_part(&db, 1, 2, 0, &mut Work::default()).unwrap(),
            Some(Ordering::Less)
        );
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

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    db.query_row("SELECT phase,next_atom,atom,minimum,compare_atom,compare_part,peer_after FROM native_physical_profiles WHERE root=?1", [root.0.as_slice()], |r| Ok(State {
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
        "SELECT original FROM native_physical_profiles WHERE root=?1",
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
fn atom_info(db: &Connection, atom: i64) -> rusqlite::Result<(i64, Vec<u8>, bool)> {
    db.query_row(
        "SELECT length,digest,complete FROM native_physical_atoms WHERE atom=?1",
        [atom],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
}
fn part(db: &Connection, atom: i64, index: i64, work: &mut Work) -> io::Result<Vec<u8>> {
    let bytes: Option<Vec<u8>> = db
        .query_row(
            "SELECT bytes FROM native_physical_atom_parts WHERE atom=?1 AND part=?2",
            params![atom, index],
            |r| r.get(0),
        )
        .optional()
        .map_err(io::Error::other)?;
    work.decoded_rows += usize::from(bytes.is_some());
    let bytes = bytes.unwrap_or_default();
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
    /// Original native evidence owns extraction and scalar semantics. Its
    /// existing job/source admission qualifies this exact immutable source;
    /// the separate Current ledger may be absent or newer throughout repair.
    pub(crate) fn physical_profile_begin_original(
        &self,
        job: &Job,
        witness: &SourceWitness,
        first: Option<i64>,
        role: PhysicalRole,
        scan_order: NativeScanOrder,
    ) -> io::Result<(ProfileRoot, ProfileProgress)> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("Original physical runtime changed"));
        }
        if PhysicalRole::recorded(&witness.source) != Some(role)
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
        if !job_current(&tx, job).map_err(io::Error::other)?
            || !check_source(&tx, witness).map_err(io::Error::other)?
        {
            return Err(io::Error::other("Original physical job/source changed"));
        }
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
        let root = ProfileRoot(digest(
            &serde_json::to_vec(&("original", source, &descriptor)).map_err(io::Error::other)?,
        ));
        tx.execute("INSERT OR IGNORE INTO native_physical_profiles(root,path,revision,descriptor,phase,original) VALUES(?1,?2,0,?3,'input',?4)",params![root.0.as_slice(),witness.source.path.to_str(),descriptor,source.as_slice()]).map_err(io::Error::other)?;
        tx.execute("INSERT INTO native_physical_bindings VALUES(?1,?2,?3) ON CONFLICT(source) DO UPDATE SET root=excluded.root,first_ms=excluded.first_ms",params![source.as_slice(),root.0.as_slice(),first]).map_err(io::Error::other)?;
        let progress = progress(&tx, root)?;
        tx.commit().map_err(io::Error::other)?;
        if !self.source_witness_current(witness)? {
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
        let (kind, native) = if ordinal == 0 {
            ("own".to_owned(), witness.source.native_id.clone())
        } else {
            if role != PhysicalRole::Top {
                self.physical_profile_finish(witness, root, ordinal)?;
                return Ok((root, ProfileProgress::Ready, work));
            }
            let saved:Option<(i64,String,String)>=self.connection.query_row("SELECT ordinal,kind,native_id FROM native_physical_current_cursors WHERE root=?1",[root.0.as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(io::Error::other)?;
            if let Some((saved_ordinal, kind, native)) = &saved
                && u64::try_from(*saved_ordinal).ok() == Some(ordinal)
            {
                (kind.clone(), native.clone())
            } else {
                let after = saved
                    .as_ref()
                    .map(|(_, kind, native)| (kind.as_str(), native.as_str()));
                let page = match self.links_page(witness, after) {
                    Ok(page) => page,
                    Err(error) if error.to_string().contains("exceeds metadata budget") => {
                        return Ok((root, ProfileProgress::NativeAtomsRequired, work));
                    }
                    Err(error) => return Err(error),
                };
                work = page.work;
                let Some((kind, native)) = page.rows.into_iter().next() else {
                    self.physical_profile_finish(witness, root, ordinal)?;
                    return Ok((root, ProfileProgress::Ready, work));
                };
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
        let header = self
            .source_header(witness)?
            .ok_or_else(|| io::Error::other("physical source header requires Original producer"))?;
        if !header.primitives_ready {
            return Err(io::Error::other(
                "physical source primitives require bounded repair",
            ));
        }
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
        let root = ProfileRoot(digest(
            &serde_json::to_vec(&(path, revision, &descriptor)).map_err(io::Error::other)?,
        ));
        tx.execute("INSERT OR IGNORE INTO native_physical_profiles(root,path,revision,descriptor,phase) VALUES(?1,?2,?3,?4,'input')",params![root.0.as_slice(),path,revision,descriptor]).map_err(io::Error::other)?;
        ensure_build(&tx, witness, root).map_err(io::Error::other)?;
        tx.execute("INSERT INTO native_physical_bindings VALUES(?1,?2,?3) ON CONFLICT(source) DO UPDATE SET root=excluded.root,first_ms=excluded.first_ms",params![source_key(&witness.source)?.as_slice(),root.0.as_slice(),header.first]).map_err(io::Error::other)?;
        let progress = progress(&tx, root)?;
        tx.commit().map_err(io::Error::other)?;
        if !self.source_witness_current(witness)? {
            return Err(io::Error::other(
                "physical source changed after profile begin",
            ));
        }
        Ok((root, progress))
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
        if bytes.len() > PART || (!last && bytes.len() != PART) || (bytes.is_empty() && offset != 0)
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
        let s = state(&tx, root).map_err(io::Error::other)?;
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
            "UPDATE native_physical_atoms SET length=length+?2,digest=?3,complete=?4 WHERE atom=?1",
            params![atom, bytes.len() as i64, digest.as_slice(), last],
        )
        .map_err(io::Error::other)?;
        if last {
            tx.execute("UPDATE native_physical_profiles SET phase='minimum',compare_atom=minimum,compare_part=0,peer_after=0,continued=continued+?2 WHERE root=?1",params![root.0.as_slice(),kind==AtomKind::Continued]).map_err(io::Error::other)?;
        }
        let progress = progress(&tx, root)?;
        tx.commit().map_err(io::Error::other)?;
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
        let mut work = Work::default();
        if cancelled() {
            return Ok((ProfileProgress::Pending, work));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        ensure_build(&tx, witness, root).map_err(io::Error::other)?;
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
        tx.commit().map_err(io::Error::other)?;
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
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        ensure_build(&tx, witness, root).map_err(io::Error::other)?;
        let s = state(&tx, root).map_err(io::Error::other)?;
        if s.phase != "input"
            || s.atom.is_some()
            || atoms == 0
            || u64::try_from(s.next).ok() != Some(atoms)
            || s.minimum.is_none()
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
        if !self.source_witness_current(witness)? {
            return Err(io::Error::other(
                "physical source changed after profile finish",
            ));
        }
        Ok(())
    }

    /// Only completed profile scalar observations are admitted by readers.
    /// Exact skipped filename probes do not visit growing session link sets.
    pub(crate) fn physical_profile(
        &self,
        witness: &SourceWitness,
        skipped: &[String],
    ) -> io::Result<PhysicalSource> {
        if !self.source_witness_current(witness)? {
            return Err(io::Error::other("physical profile witness changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let (root,first,descriptor,continued):(Vec<u8>,Option<i64>,String,i64)=tx.query_row("SELECT p.root,b.first_ms,p.descriptor,p.continued FROM native_physical_bindings b JOIN native_physical_profiles p ON p.root=b.root WHERE b.source=?1 AND p.phase='ready'",[source_key(&witness.source)?.as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(io::Error::other)?;
        let root = ProfileRoot(
            root.try_into()
                .map_err(|_| io::Error::other("invalid physical root"))?,
        );
        let descriptor: Descriptor = serde_json::from_str(&descriptor).map_err(io::Error::other)?;
        let mut truncated = false;
        for id in skipped {
            // Skipped IDs come from physical file stems, rather than arbitrary
            // giant native atoms. Still compare every part exactly on a hash hit.
            let mut chain = [0u8; 32];
            for bytes in id.as_bytes().chunks(PART) {
                let mut hash = Sha256::new();
                hash.update(chain);
                hash.update(bytes);
                chain = hash.finalize().into();
            }
            if id.is_empty() {
                let mut hash = Sha256::new();
                hash.update(chain);
                chain = hash.finalize().into();
            }
            let mut q=tx.prepare("SELECT atom FROM native_physical_atoms WHERE root=?1 AND kind='session' AND digest=?2 AND length=?3 AND complete=1").map_err(io::Error::other)?;
            let atoms = q
                .query_map(
                    params![
                        root.0.as_slice(),
                        chain.as_slice(),
                        sequence_sql(id.len()).map_err(io::Error::other)?
                    ],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(io::Error::other)?;
            for atom in atoms {
                let atom = atom.map_err(io::Error::other)?;
                let mut equal = true;
                for (i, bytes) in id.as_bytes().chunks(PART).enumerate() {
                    if part(&tx, atom, i as i64, &mut Work::default())? != bytes {
                        equal = false;
                        break;
                    }
                }
                if equal {
                    truncated = true;
                    break;
                }
            }
            if truncated {
                break;
            }
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
            turns_truncated: truncated,
        })
    }

    pub(crate) fn physical_pair(&self, left: ProfileRoot, right: ProfileRoot) -> io::Result<bool> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        for root in [left, right] {
            if state(&tx, root).map_err(io::Error::other)?.phase != "ready" {
                return Err(io::Error::other("physical pair profile is incomplete"));
            }
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
        let (left, right) = (
            state(&tx, left).map_err(io::Error::other)?,
            state(&tx, right).map_err(io::Error::other)?,
        );
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
        let a = state(&tx, low).map_err(io::Error::other)?;
        let b = state(&tx, high).map_err(io::Error::other)?;
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
        let order: Option<i64> = self
            .connection
            .query_row(
                "SELECT ordering FROM native_physical_orders WHERE left_root=?1 AND right_root=?2",
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
