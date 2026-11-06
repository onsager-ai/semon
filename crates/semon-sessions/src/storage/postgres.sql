-- Provision with a role distinct from the restricted application role. The host
-- grants only USAGE and DML to that role, never schema/table ownership.
CREATE SCHEMA semon_projection;
CREATE TABLE semon_projection.metadata (
    singleton boolean PRIMARY KEY CHECK (singleton),
    stamp text NOT NULL
);
CREATE TABLE semon_projection.namespaces (
    namespace text PRIMARY KEY,
    epoch bigint NOT NULL DEFAULT 0 CHECK (epoch >= 0)
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
