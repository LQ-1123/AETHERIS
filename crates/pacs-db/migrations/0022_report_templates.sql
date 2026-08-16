-- Structured report templates (seed) and template payload snapshot on reports.
--
-- 不变量（见设计文档 2026-08-16-structured-report-workflow-design.md §3）：
--   I1 报告的渲染/编辑只依赖 diagnostic_reports.template_payload 自包含快照，
--      绝不查询本表渲染历史报告；模板后续增删改不影响已存在报告。
--   I4 模板内字段 ID 单调分配、一经使用永不复用；种子字段 ID 稳定不变。
--   I5 section.id 为固定枚举 {findings, impression, recommendation}。

CREATE TABLE report_templates (
    id             UUID PRIMARY KEY,
    institution_id BIGINT NOT NULL REFERENCES institutions(id),
    name           TEXT NOT NULL,
    modality       TEXT NOT NULL,
    body_part      TEXT,
    version        INTEGER NOT NULL DEFAULT 1,
    structure      JSONB NOT NULL,
    builtin        BOOLEAN NOT NULL DEFAULT false,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT report_templates_modality_len
        CHECK (length(btrim(modality)) BETWEEN 1 AND 16),
    CONSTRAINT report_templates_name_len
        CHECK (length(btrim(name)) BETWEEN 1 AND 120)
);

CREATE INDEX report_templates_institution_idx
    ON report_templates(institution_id, modality);

CREATE TRIGGER report_templates_set_updated_at BEFORE UPDATE ON report_templates
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

ALTER TABLE diagnostic_reports
    ADD COLUMN template_payload JSONB;

-- 种子模板：为迁移时已存在的每个机构各插一份。内容为示例结构，
-- 标注「示例模板，请按科室实际用语调整」；本表后续管理 UI 落地前不提供编辑入口。
INSERT INTO report_templates (id, institution_id, name, modality, body_part, structure, builtin)
SELECT gen_random_uuid(), i.id, 'CT-头颅', 'CT', 'head', $tpl${
  "schema_version": 1,
  "sections": [
    {
      "id": "findings",
      "title": "影像所见",
      "fields": [
        {"id": "f1", "kind": "text", "label": "整体描述", "required": true},
        {"id": "f2", "kind": "choice", "label": "脑实质", "options": [
          {"id": "normal", "label": "未见明显异常"},
          {"id": "abnormal", "label": "异常（展开描述）", "expands": true}
        ]},
        {"id": "f3", "kind": "choice", "label": "脑室系统", "options": [
          {"id": "normal", "label": "未见明显异常"},
          {"id": "abnormal", "label": "异常（展开描述）", "expands": true}
        ]}
      ]
    },
    {
      "id": "impression",
      "title": "诊断意见",
      "fields": [
        {"id": "i1", "kind": "text", "label": "诊断意见", "required": true}
      ]
    },
    {
      "id": "recommendation",
      "title": "建议",
      "fields": [
        {"id": "r1", "kind": "text", "label": "建议"}
      ]
    }
  ]
}$tpl$::jsonb, true FROM institutions i;

INSERT INTO report_templates (id, institution_id, name, modality, body_part, structure, builtin)
SELECT gen_random_uuid(), i.id, 'CT-胸部', 'CT', 'chest', $tpl${
  "schema_version": 1,
  "sections": [
    {
      "id": "findings",
      "title": "影像所见",
      "fields": [
        {"id": "f1", "kind": "text", "label": "整体描述", "required": true},
        {"id": "f2", "kind": "choice", "label": "肺实质", "options": [
          {"id": "normal", "label": "未见明显异常"},
          {"id": "abnormal", "label": "异常（展开描述）", "expands": true}
        ]},
        {"id": "f3", "kind": "number", "label": "最大结节径", "unit": "mm", "min": 0, "max": 300},
        {"id": "f4", "kind": "choice", "label": "纵隔", "options": [
          {"id": "normal", "label": "未见明显异常"},
          {"id": "abnormal", "label": "异常（展开描述）", "expands": true}
        ]}
      ]
    },
    {
      "id": "impression",
      "title": "诊断意见",
      "fields": [
        {"id": "i1", "kind": "text", "label": "诊断意见", "required": true}
      ]
    },
    {
      "id": "recommendation",
      "title": "建议",
      "fields": [
        {"id": "r1", "kind": "text", "label": "建议"}
      ]
    }
  ]
}$tpl$::jsonb, true FROM institutions i;

INSERT INTO report_templates (id, institution_id, name, modality, body_part, structure, builtin)
SELECT gen_random_uuid(), i.id, 'CT-腹部', 'CT', 'abdomen', $tpl${
  "schema_version": 1,
  "sections": [
    {
      "id": "findings",
      "title": "影像所见",
      "fields": [
        {"id": "f1", "kind": "text", "label": "整体描述", "required": true},
        {"id": "f2", "kind": "choice", "label": "实质脏器", "options": [
          {"id": "normal", "label": "未见明显异常"},
          {"id": "abnormal", "label": "异常（展开描述）", "expands": true}
        ]},
        {"id": "f3", "kind": "choice", "label": "空腔脏器", "options": [
          {"id": "normal", "label": "未见明显异常"},
          {"id": "abnormal", "label": "异常（展开描述）", "expands": true}
        ]},
        {"id": "f4", "kind": "choice", "label": "腹膜后", "options": [
          {"id": "normal", "label": "未见明显异常"},
          {"id": "abnormal", "label": "异常（展开描述）", "expands": true}
        ]}
      ]
    },
    {
      "id": "impression",
      "title": "诊断意见",
      "fields": [
        {"id": "i1", "kind": "text", "label": "诊断意见", "required": true}
      ]
    },
    {
      "id": "recommendation",
      "title": "建议",
      "fields": [
        {"id": "r1", "kind": "text", "label": "建议"}
      ]
    }
  ]
}$tpl$::jsonb, true FROM institutions i;

INSERT INTO report_templates (id, institution_id, name, modality, body_part, structure, builtin)
SELECT gen_random_uuid(), i.id, 'MR-头颅', 'MR', 'head', $tpl${
  "schema_version": 1,
  "sections": [
    {
      "id": "findings",
      "title": "影像所见",
      "fields": [
        {"id": "f1", "kind": "text", "label": "整体描述", "required": true},
        {"id": "f2", "kind": "choice", "label": "脑实质信号", "options": [
          {"id": "normal", "label": "未见明显异常"},
          {"id": "abnormal", "label": "异常（展开描述）", "expands": true}
        ]},
        {"id": "f3", "kind": "choice", "label": "脑室系统", "options": [
          {"id": "normal", "label": "未见明显异常"},
          {"id": "abnormal", "label": "异常（展开描述）", "expands": true}
        ]},
        {"id": "f4", "kind": "choice", "label": "DWI", "options": [
          {"id": "normal", "label": "未见明显弥散受限"},
          {"id": "abnormal", "label": "异常（展开描述）", "expands": true}
        ]}
      ]
    },
    {
      "id": "impression",
      "title": "诊断意见",
      "fields": [
        {"id": "i1", "kind": "text", "label": "诊断意见", "required": true}
      ]
    },
    {
      "id": "recommendation",
      "title": "建议",
      "fields": [
        {"id": "r1", "kind": "text", "label": "建议"}
      ]
    }
  ]
}$tpl$::jsonb, true FROM institutions i;

INSERT INTO report_templates (id, institution_id, name, modality, body_part, structure, builtin)
SELECT gen_random_uuid(), i.id, 'DR-胸部', 'DR', 'chest', $tpl${
  "schema_version": 1,
  "sections": [
    {
      "id": "findings",
      "title": "影像所见",
      "fields": [
        {"id": "f1", "kind": "text", "label": "整体描述", "required": true},
        {"id": "f2", "kind": "choice", "label": "肺野", "options": [
          {"id": "normal", "label": "未见明显异常"},
          {"id": "abnormal", "label": "异常（展开描述）", "expands": true}
        ]},
        {"id": "f3", "kind": "choice", "label": "心影", "options": [
          {"id": "normal", "label": "未见明显异常"},
          {"id": "abnormal", "label": "异常（展开描述）", "expands": true}
        ]},
        {"id": "f4", "kind": "choice", "label": "骨性胸廓", "options": [
          {"id": "normal", "label": "未见明显异常"},
          {"id": "abnormal", "label": "异常（展开描述）", "expands": true}
        ]}
      ]
    },
    {
      "id": "impression",
      "title": "诊断意见",
      "fields": [
        {"id": "i1", "kind": "text", "label": "诊断意见", "required": true}
      ]
    },
    {
      "id": "recommendation",
      "title": "建议",
      "fields": [
        {"id": "r1", "kind": "text", "label": "建议"}
      ]
    }
  ]
}$tpl$::jsonb, true FROM institutions i;
