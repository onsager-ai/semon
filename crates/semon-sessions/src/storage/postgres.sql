-- Provision with a role distinct from the restricted application role. The host
-- grants only USAGE and DML to that role, never schema/table ownership.
CREATE SCHEMA semon_projection;
CREATE TABLE semon_projection.namespaces (
    namespace text PRIMARY KEY,
    stamp text NOT NULL,
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
