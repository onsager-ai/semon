-- Provision with a role distinct from the restricted application role. The host
-- grants only USAGE and DML to that role, never schema/table ownership.
CREATE SCHEMA semon_projection;
CREATE TABLE semon_projection.metadata (
    singleton boolean PRIMARY KEY CHECK (singleton),
    stamp text NOT NULL
);
CREATE TABLE semon_projection.namespaces (
    namespace text PRIMARY KEY,
    epoch bigint NOT NULL DEFAULT 0 CHECK (epoch >= 0),
    custody_epoch text NOT NULL DEFAULT md5(random()::text || clock_timestamp()::text)
);
CREATE TABLE semon_projection.ledgers (
    namespace text NOT NULL REFERENCES semon_projection.namespaces(namespace),
    path text NOT NULL,
    dev bigint NOT NULL,
    ino bigint NOT NULL,
    consumed bigint NOT NULL CHECK (consumed >= 0),
    prefix bytea NOT NULL CHECK (octet_length(prefix) = 32),
    tail bytea NOT NULL CHECK (octet_length(tail) = 32),
    epoch bigint NOT NULL CHECK (epoch >= 0),
    projector integer NOT NULL CHECK (projector >= 0),
    PRIMARY KEY (namespace,path)
);
CREATE TABLE semon_projection.units (
    namespace text NOT NULL REFERENCES semon_projection.namespaces(namespace),
    unit_key text NOT NULL,
    path text NOT NULL,
    dev bigint NOT NULL,
    ino bigint NOT NULL,
    consumed bigint NOT NULL CHECK (consumed >= 0),
    prefix bytea NOT NULL CHECK (octet_length(prefix) = 32),
    tail bytea NOT NULL CHECK (octet_length(tail) = 32),
    epoch bigint NOT NULL CHECK (epoch >= 0),
    projector integer NOT NULL CHECK (projector >= 0),
    PRIMARY KEY (namespace,unit_key,path)
);
-- Metadata and normalized records commit with their consumed ledger. Native
-- bodies never enter these tables. Proofs are keyed by the exact ledger, so
-- published old prefixes survive an index advance.
CREATE TABLE semon_projection.files (
    namespace text NOT NULL REFERENCES semon_projection.namespaces(namespace),
    path text NOT NULL,
    ledger text NOT NULL,
    metadata text NOT NULL,
    PRIMARY KEY (namespace,path)
);
CREATE TABLE semon_projection.records (
    namespace text NOT NULL,
    path text NOT NULL,
    kind smallint NOT NULL CHECK (kind BETWEEN 0 AND 2),
    ordinal bigint NOT NULL CHECK (ordinal >= 0),
    record text NOT NULL,
    PRIMARY KEY (namespace,path,kind,ordinal),
    FOREIGN KEY (namespace,path) REFERENCES semon_projection.files(namespace,path) ON DELETE CASCADE
);
CREATE TABLE semon_projection.proofs (
    namespace text NOT NULL REFERENCES semon_projection.namespaces(namespace),
    path text NOT NULL,
    generation text NOT NULL,
    consumed bigint NOT NULL CHECK (consumed >= 0),
    prefix bytea NOT NULL CHECK (octet_length(prefix) = 32),
    tail bytea NOT NULL CHECK (octet_length(tail) = 32),
    resume text,
    blocks bigint NOT NULL CHECK (blocks >= 0),
    root bytea NOT NULL CHECK (octet_length(root) = 32),
    PRIMARY KEY (namespace,path,generation)
);
CREATE TABLE semon_projection.proof_blocks (
    namespace text NOT NULL,
    path text NOT NULL,
    generation text NOT NULL,
    ordinal bigint NOT NULL CHECK (ordinal >= 0),
    digest bytea NOT NULL CHECK (octet_length(digest) = 32),
    PRIMARY KEY (namespace,path,generation,ordinal),
    FOREIGN KEY (namespace,path,generation) REFERENCES semon_projection.proofs(namespace,path,generation) ON DELETE CASCADE
);
CREATE TABLE semon_projection.proof_nodes (
    namespace text NOT NULL,
    path text NOT NULL,
    generation text NOT NULL,
    level integer NOT NULL CHECK (level >= 0),
    ordinal bigint NOT NULL CHECK (ordinal >= 0),
    digest bytea NOT NULL CHECK (octet_length(digest) = 32),
    PRIMARY KEY (namespace,path,generation,level,ordinal),
    FOREIGN KEY (namespace,path,generation) REFERENCES semon_projection.proofs(namespace,path,generation) ON DELETE CASCADE
);
CREATE TABLE semon_projection.reported_facts (
    namespace text PRIMARY KEY REFERENCES semon_projection.namespaces(namespace),
    revision bigint NOT NULL CHECK (revision >= 0),
    metadata text NOT NULL CHECK (octet_length(metadata) <= 1048576)
);
CREATE TABLE semon_projection.checkpoints (
    namespace text NOT NULL REFERENCES semon_projection.namespaces(namespace),
    checkpoint_key text NOT NULL CHECK (octet_length(checkpoint_key) BETWEEN 1 AND 4096),
    revision bigint NOT NULL CHECK (revision >= 0),
    metadata text NOT NULL CHECK (octet_length(metadata) <= 1048576),
    PRIMARY KEY (namespace,checkpoint_key)
);
CREATE TABLE semon_projection.raw_sources (
    namespace text NOT NULL REFERENCES semon_projection.namespaces(namespace),
    root text NOT NULL CHECK (root IN ('claude','codex','copilot')),
    path text NOT NULL CHECK (octet_length(path) BETWEEN 1 AND 4096),
    revision bigint NOT NULL CHECK (revision > 0),
    pending boolean NOT NULL,
    intent_id text NOT NULL DEFAULT gen_random_uuid()::text,
    generation text CHECK (octet_length(generation) <= 1048576),
    CHECK (pending OR generation IS NOT NULL),
    PRIMARY KEY (namespace,root,path)
);
ALTER TABLE semon_projection.raw_sources ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.raw_sources FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.raw_sources
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
CREATE TABLE semon_projection.change_heads (
    namespace text NOT NULL REFERENCES semon_projection.namespaces(namespace),
    read_scope text NOT NULL CHECK (read_scope IN ('current','retained_history')),
    head bigint NOT NULL CHECK (head >= 0),
    floor bigint NOT NULL CHECK (floor >= 0 AND floor <= head),
    PRIMARY KEY (namespace,read_scope)
);
CREATE TABLE semon_projection.changes (
    namespace text NOT NULL,
    read_scope text NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    session_key text NOT NULL CHECK (octet_length(session_key) BETWEEN 1 AND 4096),
    PRIMARY KEY (namespace,read_scope,revision),
    FOREIGN KEY (namespace,read_scope) REFERENCES semon_projection.change_heads(namespace,read_scope)
);
ALTER TABLE semon_projection.reported_facts ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.reported_facts FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.reported_facts
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.checkpoints ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.checkpoints FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.checkpoints
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.change_heads ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.change_heads FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.change_heads
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.changes ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.changes FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.changes
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
CREATE TABLE semon_projection.catalog_heads (
    namespace text PRIMARY KEY REFERENCES semon_projection.namespaces(namespace),
    generation text NOT NULL
);
CREATE TABLE semon_projection.catalog (
    namespace text NOT NULL REFERENCES semon_projection.namespaces(namespace),
    session_key text NOT NULL,
    lifecycle text NOT NULL CHECK (lifecycle IN ('current','retained')),
    last_ms bigint NOT NULL,
    harness text NOT NULL,
    repo text NOT NULL,
    model text NOT NULL,
    metadata text NOT NULL CHECK (octet_length(metadata) <= 1048576),
    PRIMARY KEY (namespace,session_key)
);
CREATE INDEX catalog_current_order ON semon_projection.catalog(namespace,last_ms DESC,session_key) WHERE lifecycle='current';
CREATE INDEX catalog_history_order ON semon_projection.catalog(namespace,last_ms DESC,session_key);
CREATE TABLE semon_projection.search_postings (
    namespace text NOT NULL,
    session_key text NOT NULL,
    read_scope text NOT NULL CHECK (read_scope IN ('current','retained_history')),
    gram text NOT NULL,
    last_ms bigint NOT NULL,
    PRIMARY KEY (namespace,session_key,read_scope,gram),
    FOREIGN KEY (namespace,session_key) REFERENCES semon_projection.catalog(namespace,session_key)
);
CREATE INDEX search_posting_order ON semon_projection.search_postings(namespace,read_scope,gram,last_ms DESC,session_key);
ALTER TABLE semon_projection.search_postings ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.search_postings FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.search_postings
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
CREATE TABLE semon_projection.recipe_heads (
    namespace text NOT NULL,
    session_key text NOT NULL,
    generation text NOT NULL,
    source_generation text NOT NULL,
    version integer NOT NULL,
    total bigint NOT NULL CHECK (total >= 0),
    PRIMARY KEY (namespace,session_key),
    FOREIGN KEY (namespace,session_key) REFERENCES semon_projection.catalog(namespace,session_key)
);
CREATE TABLE semon_projection.recipes (
    namespace text NOT NULL,
    session_key text NOT NULL,
    slot bigint NOT NULL CHECK (slot >= 0),
    metadata text NOT NULL CHECK (octet_length(metadata) <= 1048576),
    PRIMARY KEY (namespace,session_key,slot),
    FOREIGN KEY (namespace,session_key) REFERENCES semon_projection.recipe_heads(namespace,session_key)
);
CREATE TABLE semon_projection.history_catalog (LIKE semon_projection.catalog INCLUDING CONSTRAINTS INCLUDING INDEXES);
CREATE TABLE semon_projection.history_recipe_heads (LIKE semon_projection.recipe_heads INCLUDING CONSTRAINTS INCLUDING INDEXES);
CREATE TABLE semon_projection.history_recipes (LIKE semon_projection.recipes INCLUDING CONSTRAINTS INCLUDING INDEXES);
ALTER TABLE semon_projection.catalog_heads ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.catalog_heads FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.catalog_heads
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.catalog ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.catalog FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.catalog
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.recipe_heads ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.recipe_heads FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.recipe_heads
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.recipes ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.recipes FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.recipes
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.history_catalog ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.history_catalog FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.history_catalog
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.history_recipe_heads ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.history_recipe_heads FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.history_recipe_heads
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.history_recipes ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.history_recipes FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.history_recipes
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.proof_nodes ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.proof_nodes FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.proof_nodes
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.files ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.files FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.files
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.records ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.records FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.records
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.proofs ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.proofs FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.proofs
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.proof_blocks ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.proof_blocks FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.proof_blocks
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.namespaces ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.namespaces FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.namespaces
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.ledgers ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.ledgers FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.ledgers
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
ALTER TABLE semon_projection.units ENABLE ROW LEVEL SECURITY;
ALTER TABLE semon_projection.units FORCE ROW LEVEL SECURITY;
CREATE POLICY namespace_isolation ON semon_projection.units
    USING (namespace = current_setting('semon.namespace',true))
    WITH CHECK (namespace = current_setting('semon.namespace',true));
