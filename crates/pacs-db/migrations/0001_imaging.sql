-- 影像四层结构:patients → studies → series → instances,外键级联删除。
--
-- 每层都是「结构化列 + 一列 JSONB」:
--   * 结构化列进索引,负责 C-FIND 匹配和 QIDO-RS 查询;
--   * attributes 存该层的 DICOM JSON Model 属性子集,新增查询键时不用改表。
--
-- 命名约定:外键叫 <表名单数>_fk,DICOM 属性列保留标准关键字的名字。
-- 否则 patients.patient_id (0010,0020) 会和"指向 patients 的外键"撞名 ——
-- dcm4chee 用的也是这个约定。

CREATE FUNCTION set_updated_at() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$;

-- 多租户预留。第一版只有一个机构,但所有影像表都带 institution_id ——
-- 后期再加会牵动每一条查询,现在留下几乎零成本。
CREATE TABLE institutions (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    code        TEXT        NOT NULL UNIQUE,
    name        TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO institutions (code, name) VALUES ('default', '默认机构');


CREATE TABLE patients (
    id                    BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    institution_id        BIGINT      NOT NULL DEFAULT 1 REFERENCES institutions(id),
    -- (0010,0020)。DICOM 里是 Type 2,允许存在但为空;空串表示设备没提供。
    -- 已知局限:同机构下所有"无 ID"的病人会归到同一条记录。真实医院环境里
    -- PatientID 一定有值,等接入 HIS 后再按 IssuerOfPatientID 细分。
    patient_id            TEXT        NOT NULL,
    issuer_of_patient_id  TEXT,
    name                  TEXT,                    -- (0010,0010) 原始 PN
    name_normalized       TEXT,                    -- 匹配用:字母组、去尾部空分量、大写
    birth_date            DATE,                    -- (0010,0030)
    sex                   TEXT,                    -- (0010,0040)
    attributes            JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (institution_id, patient_id)
);

-- text_pattern_ops 让 LIKE 'ZHANG%' 能走索引,不受数据库排序规则影响。
-- C-FIND 的姓名通配查询全靠它。
CREATE INDEX patients_name_normalized_idx ON patients (name_normalized text_pattern_ops);

CREATE TRIGGER patients_set_updated_at BEFORE UPDATE ON patients
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();


CREATE TABLE studies (
    id                   BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    patient_fk           BIGINT      NOT NULL REFERENCES patients(id) ON DELETE CASCADE,
    institution_id       BIGINT      NOT NULL DEFAULT 1 REFERENCES institutions(id),
    study_instance_uid   TEXT        NOT NULL UNIQUE,   -- (0020,000D)
    study_date           DATE,                          -- (0008,0020)
    study_time           TIME,                          -- (0008,0030)
    accession_number     TEXT,                          -- (0008,0050)
    study_id             TEXT,                          -- (0020,0010)
    description          TEXT,                          -- (0008,1030)
    referring_physician  TEXT,                          -- (0008,0090)
    -- 以下三列由 series/instances 聚合而来,是 C-FIND 的返回键
    -- (ModalitiesInStudy / NumberOfStudyRelatedSeries / ...Instances)。
    -- 每次入库在同一事务里按实际行数重算,不做增量累加 ——
    -- 增量在重传和回滚下会漂移,而重算的代价只是一次索引扫描。
    modalities           TEXT[]      NOT NULL DEFAULT '{}',
    number_of_series     INTEGER     NOT NULL DEFAULT 0,
    number_of_instances  INTEGER     NOT NULL DEFAULT 0,
    attributes           JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX studies_patient_fk_idx ON studies (patient_fk);
CREATE INDEX studies_study_date_idx ON studies (study_date);
CREATE INDEX studies_accession_idx ON studies (accession_number text_pattern_ops);
CREATE INDEX studies_modalities_idx ON studies USING GIN (modalities);

CREATE TRIGGER studies_set_updated_at BEFORE UPDATE ON studies
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();


CREATE TABLE series (
    id                   BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    study_fk             BIGINT      NOT NULL REFERENCES studies(id) ON DELETE CASCADE,
    series_instance_uid  TEXT        NOT NULL UNIQUE,   -- (0020,000E)
    series_number        INTEGER,                       -- (0020,0011)
    modality             TEXT,                          -- (0008,0060)
    description          TEXT,                          -- (0008,103E)
    body_part_examined   TEXT,                          -- (0018,0015)
    series_date          DATE,                          -- (0008,0021)
    series_time          TIME,                          -- (0008,0031)
    number_of_instances  INTEGER     NOT NULL DEFAULT 0,
    attributes           JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX series_study_fk_idx ON series (study_fk);
CREATE INDEX series_modality_idx ON series (modality);

CREATE TRIGGER series_set_updated_at BEFORE UPDATE ON series
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();


CREATE TABLE instances (
    id                   BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    series_fk            BIGINT      NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    sop_instance_uid     TEXT        NOT NULL UNIQUE,   -- (0008,0018)
    sop_class_uid        TEXT,                          -- (0008,0016)
    instance_number      INTEGER,                       -- (0020,0013)
    -- 决定像素数据怎么解码,取自文件元信息而非数据集
    transfer_syntax_uid  TEXT        NOT NULL,
    image_rows           INTEGER,                       -- (0028,0010)
    image_columns        INTEGER,                       -- (0028,0011)
    number_of_frames     INTEGER,                       -- (0028,0008)
    -- CT 序列排序要把这两个值投影到切片法向量上算,不能用 InstanceNumber。
    image_position_patient     DOUBLE PRECISION[],      -- (0020,0032)
    image_orientation_patient  DOUBLE PRECISION[],      -- (0020,0037)
    -- 相对存储根的路径。存相对值,整个存储根才能整体迁移。
    storage_path         TEXT        NOT NULL,
    file_size            BIGINT      NOT NULL,
    file_sha256          BYTEA       NOT NULL,
    attributes           JSONB       NOT NULL DEFAULT '{}'::jsonb,
    received_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX instances_series_fk_idx ON instances (series_fk);

CREATE TRIGGER instances_set_updated_at BEFORE UPDATE ON instances
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
