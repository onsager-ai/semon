-- Materialize as conversations/fixture-cascade.db.
-- ACP 1.5.18 metadata projection. All values are synthetic.
CREATE TABLE trajectory_meta (
    trajectory_id TEXT PRIMARY KEY,
    cascade_id TEXT,
    trajectory_type INTEGER,
    source INTEGER
);
CREATE TABLE steps (
    idx INTEGER PRIMARY KEY,
    step_type INTEGER,
    status INTEGER,
    has_subtrajectory NUMERIC
);
CREATE INDEX idx_steps_step_type ON steps (step_type);
CREATE INDEX idx_steps_status ON steps (status);

INSERT INTO trajectory_meta VALUES ('fixture-trajectory', 'fixture-cascade', 0, 0);
INSERT INTO steps VALUES (0, 31, 1, 0), (1, 8, 2, 1), (2, 31, 1, 0);
