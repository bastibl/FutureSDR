CREATE TYPE data_type_marker AS ENUM ('fft', 'zigbee');

CREATE TABLE data_storage(
                             id INTEGER PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
                             node_id UUID NOT NULL,
                             data_type data_type_marker NOT NULL,
                             freq BIGINT  NOT NULL CHECK (freq >= 1000000 AND freq <= 6000000000),
                             amp SMALLINT NOT NULL CHECK (amp = 0 OR amp = 1),
                             lna SMALLINT NOT NULL CHECK (lna >= 0 AND lna <= 40),
                             vga SMALLINT NOT NULL CHECK (vga >= 0 AND vga <= 62),
                             sample_rate BIGINT NOT NULL CHECK (sample_rate >= 1000000 AND sample_rate <= 20000000),
                             timestamp TIMESTAMPTZ NOT NULL,
                             data BYTEA NOT NULL
);

CREATE TABLE config_storage(
                               node_id UUID PRIMARY KEY,
                               last_seen TIMESTAMPTZ NOT NULL,
                               config_serialized VARCHAR NOT NULL
);
