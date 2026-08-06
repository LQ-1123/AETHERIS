//! C-FIND 查询:把 [`pacs_core::Query`] 翻成 SQL,再把结果行翻回 DICOM 响应标识符。
//!
//! # 为什么过滤键只能用「本层及以上」的列
//!
//! 四个层级各自决定 JOIN 到哪一张表。只用本层及以上的列,连接关系就始终是
//! 多对一,一行结果对应一个实体 —— 不会因为一个检查下有 300 张图而返回
//! 300 条重复的检查记录。要支持跨层过滤(比如"含 CT 序列的检查")就得
//! 加 `DISTINCT` 或子查询,而那正是 0001 迁移里 `studies.modalities`
//! 这类聚合列存在的理由:把跨层条件预先摊平到本层。
//!
//! 不支持的键按标准(PS3.4 C.2.2.1.2)忽略,并把响应状态降为 `0xFF01`
//! 告诉对方"有键没能支持" —— 静默忽略会让对方以为过滤生效了。

use chrono::{NaiveDate, NaiveTime};
use dicom::core::{DataElement, PrimitiveValue, Tag, VR};
use dicom::dictionary_std::tags;
use dicom::object::InMemDicomObject;
use pacs_core::query::{MatchKey, Query, QueryLevel, wildcard_to_sql_like};
use sqlx::{PgPool, Row};

use crate::DbError;

/// 一次 C-FIND 的结果。
#[derive(Debug, Clone)]
pub struct FindResults {
    /// 每条一个响应标识符,可直接作为 C-FIND-RSP 的数据集发出。
    pub identifiers: Vec<InMemDicomObject>,
    /// 请求里出现、但本层不支持的匹配键。非空时响应状态应为 `PENDING_WITH_WARNING`。
    pub unsupported_keys: Vec<Tag>,
}

/// 结果条数上限。
///
/// 超过就返回错误而不是截断:截断会让对方以为"就这么多",
/// 一次漏掉的检查比一次明确的失败危险得多。
pub const DEFAULT_LIMIT: usize = 10_000;

/// 列的取值类型,决定怎么绑参数、怎么读结果、怎么写回 DICOM 值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    /// Postgres `TEXT[]`,对应 DICOM 的多值(如 ModalitiesInStudy)。
    TextArray,
    Date,
    Time,
    Int,
}

/// 一个可查询/可返回的属性到数据库列的映射。
#[derive(Debug, Clone, Copy)]
struct Column {
    tag: Tag,
    vr: VR,
    /// 出现在 SELECT 里的表达式。
    select: &'static str,
    /// 出现在 WHERE 里的表达式。
    ///
    /// 与 `select` 分开是因为 PatientName:匹配要用规范化过的
    /// `name_normalized`(大写、去尾部空分量),返回却必须是原始的 `name` ——
    /// 把规范化后的名字回给对方就篡改了病人信息。
    filter: &'static str,
    kind: Kind,
    /// 该列所属的最浅层级。查询层级不低于它时这一列才可用。
    scope: QueryLevel,
}

const COLUMNS: &[Column] = &[
    // —— Patient ——
    Column {
        tag: tags::PATIENT_ID,
        vr: VR::LO,
        select: "p.patient_id",
        filter: "p.patient_id",
        kind: Kind::Text,
        scope: QueryLevel::Patient,
    },
    Column {
        tag: tags::ISSUER_OF_PATIENT_ID,
        vr: VR::LO,
        select: "p.issuer_of_patient_id",
        filter: "p.issuer_of_patient_id",
        kind: Kind::Text,
        scope: QueryLevel::Patient,
    },
    Column {
        tag: tags::PATIENT_NAME,
        vr: VR::PN,
        select: "p.name",
        filter: "p.name_normalized",
        kind: Kind::Text,
        scope: QueryLevel::Patient,
    },
    Column {
        tag: tags::PATIENT_BIRTH_DATE,
        vr: VR::DA,
        select: "p.birth_date",
        filter: "p.birth_date",
        kind: Kind::Date,
        scope: QueryLevel::Patient,
    },
    Column {
        tag: tags::PATIENT_SEX,
        vr: VR::CS,
        select: "p.sex",
        filter: "p.sex",
        kind: Kind::Text,
        scope: QueryLevel::Patient,
    },
    // —— Study ——
    Column {
        tag: tags::STUDY_INSTANCE_UID,
        vr: VR::UI,
        select: "s.study_instance_uid",
        filter: "s.study_instance_uid",
        kind: Kind::Text,
        scope: QueryLevel::Study,
    },
    Column {
        tag: tags::STUDY_DATE,
        vr: VR::DA,
        select: "s.study_date",
        filter: "s.study_date",
        kind: Kind::Date,
        scope: QueryLevel::Study,
    },
    Column {
        tag: tags::STUDY_TIME,
        vr: VR::TM,
        select: "s.study_time",
        filter: "s.study_time",
        kind: Kind::Time,
        scope: QueryLevel::Study,
    },
    Column {
        tag: tags::ACCESSION_NUMBER,
        vr: VR::SH,
        select: "s.accession_number",
        filter: "s.accession_number",
        kind: Kind::Text,
        scope: QueryLevel::Study,
    },
    Column {
        tag: tags::STUDY_ID,
        vr: VR::SH,
        select: "s.study_id",
        filter: "s.study_id",
        kind: Kind::Text,
        scope: QueryLevel::Study,
    },
    Column {
        tag: tags::STUDY_DESCRIPTION,
        vr: VR::LO,
        select: "s.description",
        filter: "s.description",
        kind: Kind::Text,
        scope: QueryLevel::Study,
    },
    Column {
        tag: tags::REFERRING_PHYSICIAN_NAME,
        vr: VR::PN,
        select: "s.referring_physician",
        filter: "s.referring_physician",
        kind: Kind::Text,
        scope: QueryLevel::Study,
    },
    Column {
        tag: tags::MODALITIES_IN_STUDY,
        vr: VR::CS,
        select: "s.modalities",
        filter: "s.modalities",
        kind: Kind::TextArray,
        scope: QueryLevel::Study,
    },
    Column {
        tag: tags::NUMBER_OF_STUDY_RELATED_SERIES,
        vr: VR::IS,
        select: "s.number_of_series",
        filter: "s.number_of_series",
        kind: Kind::Int,
        scope: QueryLevel::Study,
    },
    Column {
        tag: tags::NUMBER_OF_STUDY_RELATED_INSTANCES,
        vr: VR::IS,
        select: "s.number_of_instances",
        filter: "s.number_of_instances",
        kind: Kind::Int,
        scope: QueryLevel::Study,
    },
    // —— Series ——
    Column {
        tag: tags::SERIES_INSTANCE_UID,
        vr: VR::UI,
        select: "se.series_instance_uid",
        filter: "se.series_instance_uid",
        kind: Kind::Text,
        scope: QueryLevel::Series,
    },
    Column {
        tag: tags::SERIES_NUMBER,
        vr: VR::IS,
        select: "se.series_number",
        filter: "se.series_number",
        kind: Kind::Int,
        scope: QueryLevel::Series,
    },
    Column {
        tag: tags::MODALITY,
        vr: VR::CS,
        select: "se.modality",
        filter: "se.modality",
        kind: Kind::Text,
        scope: QueryLevel::Series,
    },
    Column {
        tag: tags::SERIES_DESCRIPTION,
        vr: VR::LO,
        select: "se.description",
        filter: "se.description",
        kind: Kind::Text,
        scope: QueryLevel::Series,
    },
    Column {
        tag: tags::BODY_PART_EXAMINED,
        vr: VR::CS,
        select: "se.body_part_examined",
        filter: "se.body_part_examined",
        kind: Kind::Text,
        scope: QueryLevel::Series,
    },
    Column {
        tag: tags::SERIES_DATE,
        vr: VR::DA,
        select: "se.series_date",
        filter: "se.series_date",
        kind: Kind::Date,
        scope: QueryLevel::Series,
    },
    Column {
        tag: tags::SERIES_TIME,
        vr: VR::TM,
        select: "se.series_time",
        filter: "se.series_time",
        kind: Kind::Time,
        scope: QueryLevel::Series,
    },
    Column {
        tag: tags::NUMBER_OF_SERIES_RELATED_INSTANCES,
        vr: VR::IS,
        select: "se.number_of_instances",
        filter: "se.number_of_instances",
        kind: Kind::Int,
        scope: QueryLevel::Series,
    },
    // —— Instance ——
    Column {
        tag: tags::SOP_INSTANCE_UID,
        vr: VR::UI,
        select: "i.sop_instance_uid",
        filter: "i.sop_instance_uid",
        kind: Kind::Text,
        scope: QueryLevel::Image,
    },
    Column {
        tag: tags::SOP_CLASS_UID,
        vr: VR::UI,
        select: "i.sop_class_uid",
        filter: "i.sop_class_uid",
        kind: Kind::Text,
        scope: QueryLevel::Image,
    },
    Column {
        tag: tags::INSTANCE_NUMBER,
        vr: VR::IS,
        select: "i.instance_number",
        filter: "i.instance_number",
        kind: Kind::Int,
        scope: QueryLevel::Image,
    },
    Column {
        tag: tags::ROWS,
        vr: VR::US,
        select: "i.image_rows",
        filter: "i.image_rows",
        kind: Kind::Int,
        scope: QueryLevel::Image,
    },
    Column {
        tag: tags::COLUMNS,
        vr: VR::US,
        select: "i.image_columns",
        filter: "i.image_columns",
        kind: Kind::Int,
        scope: QueryLevel::Image,
    },
    Column {
        tag: tags::NUMBER_OF_FRAMES,
        vr: VR::IS,
        select: "i.number_of_frames",
        filter: "i.number_of_frames",
        kind: Kind::Int,
        scope: QueryLevel::Image,
    },
];

fn column_for(tag: Tag, level: QueryLevel) -> Option<&'static Column> {
    COLUMNS
        .iter()
        .find(|column| column.tag == tag && column.scope.depth() <= level.depth())
}

/// 每层的唯一键。响应里必须带上,对方靠它下钻到下一层。
fn unique_key(level: QueryLevel) -> Tag {
    match level {
        QueryLevel::Patient => tags::PATIENT_ID,
        QueryLevel::Study => tags::STUDY_INSTANCE_UID,
        QueryLevel::Series => tags::SERIES_INSTANCE_UID,
        QueryLevel::Image => tags::SOP_INSTANCE_UID,
    }
}

fn from_clause(level: QueryLevel) -> &'static str {
    match level {
        QueryLevel::Patient => "FROM patients p",
        QueryLevel::Study => "FROM studies s JOIN patients p ON s.patient_fk = p.id",
        QueryLevel::Series => {
            "FROM series se \
             JOIN studies s ON se.study_fk = s.id \
             JOIN patients p ON s.patient_fk = p.id"
        }
        QueryLevel::Image => {
            "FROM instances i \
             JOIN series se ON i.series_fk = se.id \
             JOIN studies s ON se.study_fk = s.id \
             JOIN patients p ON s.patient_fk = p.id"
        }
    }
}

/// 一个待绑定的 SQL 参数。
#[derive(Debug, Clone)]
enum Bind {
    Text(String),
    TextList(Vec<String>),
    Date(NaiveDate),
    Time(NaiveTime),
    Int(i32),
    BigInt(i64),
}

/// 执行一次 C-FIND 查询。
pub async fn find(pool: &PgPool, query: &Query, limit: usize) -> Result<FindResults, DbError> {
    find_inner(pool, query, limit, None).await
}

/// 执行一次限定机构的查询。
///
/// DIMSE C-FIND 仍使用 [`find`]，因为设备连接目前没有用户身份；所有经过
/// HTTP 认证的 QIDO 请求必须走这个入口，机构 ID 来自已验签的 access token。
pub async fn find_for_institution(
    pool: &PgPool,
    query: &Query,
    limit: usize,
    institution_id: i64,
) -> Result<FindResults, DbError> {
    find_inner(pool, query, limit, Some(institution_id)).await
}

async fn find_inner(
    pool: &PgPool,
    query: &Query,
    limit: usize,
    institution_id: Option<i64>,
) -> Result<FindResults, DbError> {
    let level = query.level;

    // —— SELECT:请求的键 + 本层唯一键 ——
    let mut selected: Vec<&'static Column> = Vec::new();
    let mut unsupported_keys = Vec::new();
    for tag in query.keys.keys() {
        match column_for(*tag, level) {
            Some(column) => selected.push(column),
            None => unsupported_keys.push(*tag),
        }
    }
    if let Some(unique) = column_for(unique_key(level), level)
        && !selected.iter().any(|c| c.tag == unique.tag)
    {
        selected.push(unique);
    }

    // —— WHERE ——
    let mut conditions: Vec<String> = Vec::new();
    let mut binds: Vec<Bind> = Vec::new();
    if level == QueryLevel::Patient {
        conditions.push(
            "EXISTS (SELECT 1 FROM studies visible_study WHERE visible_study.patient_fk=p.id AND visible_study.storage_tier<>'quarantine')".to_owned(),
        );
    } else {
        conditions.push("s.storage_tier <> 'quarantine'".to_owned());
    }
    if let Some(institution_id) = institution_id {
        binds.push(Bind::BigInt(institution_id));
        conditions.push("p.institution_id = $1".to_owned());
        if level.depth() >= QueryLevel::Study.depth() {
            conditions.push("s.institution_id = $1".to_owned());
        }
    }
    for (tag, key) in query.filters() {
        let Some(column) = column_for(tag, level) else {
            continue; // 已记进 unsupported_keys
        };
        if let Some(condition) = build_condition(column, key, &mut binds) {
            conditions.push(condition);
        } else {
            // 匹配形式与列类型对不上(比如对整数列用通配符)。
            // 同样按"不支持的键"处理,而不是拼一条永假或永真的条件。
            unsupported_keys.push(tag);
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    // 稳定排序:同一查询重复执行结果顺序一致,否则分页和测试都没法做
    let order_by = column_for(unique_key(level), level).map_or("1", |column| column.select);
    // 多取一条用来判断是否越界 —— 越界要报错,不能悄悄截断
    let sql = format!(
        "SELECT {} {}{} ORDER BY {order_by} LIMIT {}",
        selected
            .iter()
            .map(|c| c.select)
            .collect::<Vec<_>>()
            .join(", "),
        from_clause(level),
        where_clause,
        limit.saturating_add(1),
    );

    // sqlx 0.9 要求动态 SQL 显式声明「已人工审计过注入风险」。这里的审计结论:
    // **拼进 SQL 文本的每一段都是 `&'static str`,没有任何一个字节来自请求**。
    //   * SELECT 列表、FROM/JOIN、WHERE 里的列名 —— 全部取自上面的 `COLUMNS`
    //     常量表和 `from_clause`,是编译期字面量;
    //   * 请求带来的值一律走 `$n` 占位符 + `.bind()`,包括通配符模式
    //     (`wildcard_to_sql_like` 只做转义,产物仍然是绑定参数);
    //   * `limit` 是 `usize`,格式化出来只可能是十进制数字。
    // 改动这个函数时必须重新过一遍这条不变量。
    let mut statement = sqlx::query(sqlx::AssertSqlSafe(sql));
    for bind in &binds {
        statement = match bind {
            Bind::Text(value) => statement.bind(value),
            Bind::TextList(values) => statement.bind(values),
            Bind::Date(value) => statement.bind(value),
            Bind::Time(value) => statement.bind(value),
            Bind::Int(value) => statement.bind(value),
            Bind::BigInt(value) => statement.bind(value),
        };
    }

    let rows = statement.fetch_all(pool).await?;
    if rows.len() > limit {
        return Err(DbError::TooManyResults { limit });
    }

    let identifiers = rows
        .iter()
        .map(|row| build_identifier(row, &selected, level))
        .collect::<Result<Vec<_>, _>>()?;

    unsupported_keys.sort_unstable();
    unsupported_keys.dedup();
    Ok(FindResults {
        identifiers,
        unsupported_keys,
    })
}

/// 把一个匹配键翻成 SQL 条件,并把参数追加到 `binds`。
///
/// 返回 `None` 表示这种匹配形式对这一列没有意义,调用方按「不支持」处理。
fn build_condition(column: &Column, key: &MatchKey, binds: &mut Vec<Bind>) -> Option<String> {
    let next = |binds: &Vec<Bind>| format!("${}", binds.len() + 1);

    match (key, column.kind) {
        (MatchKey::Universal, _) => None, // 调用方已过滤掉,防御性写在这里

        // —— 文本 ——
        (MatchKey::Single(value), Kind::Text) => {
            let placeholder = next(binds);
            binds.push(Bind::Text(normalize_for(column, value)));
            Some(format!("{} = {placeholder}", column.filter))
        }
        (MatchKey::Wildcard(pattern), Kind::Text) => {
            let placeholder = next(binds);
            binds.push(Bind::Text(wildcard_to_sql_like(&normalize_for(
                column, pattern,
            ))));
            // 显式写 ESCAPE '\' 与 wildcard_to_sql_like 的转义符对应。
            // Postgres 的 LIKE 默认转义符恰好也是 `\`,所以这句在当前配置下
            // 是冗余的 —— 但写出来才不依赖那个默认值:它受
            // `standard_conforming_strings` 影响,也可能在别的部署里被改掉。
            Some(format!(r"{} LIKE {placeholder} ESCAPE '\'", column.filter))
        }
        (MatchKey::UidList(values), Kind::Text) => {
            let placeholder = next(binds);
            binds.push(Bind::TextList(values.clone()));
            Some(format!("{} = ANY({placeholder})", column.filter))
        }

        // —— 多值列:DICOM 的单值匹配语义是「包含」——
        (MatchKey::Single(value), Kind::TextArray) => {
            let placeholder = next(binds);
            binds.push(Bind::Text(value.clone()));
            Some(format!("{placeholder} = ANY({})", column.filter))
        }
        (MatchKey::UidList(values), Kind::TextArray) => {
            let placeholder = next(binds);
            binds.push(Bind::TextList(values.clone()));
            // 数组有交集即命中
            Some(format!("{} && {placeholder}", column.filter))
        }

        // —— 日期 ——
        (MatchKey::Single(value), Kind::Date) => {
            let date = pacs_core::query::parse_da(value)?;
            let placeholder = next(binds);
            binds.push(Bind::Date(date));
            Some(format!("{} = {placeholder}", column.filter))
        }
        (MatchKey::DateRange { from, to }, Kind::Date) => {
            range_condition(column.filter, *from, *to, binds, Bind::Date)
        }

        // —— 时间 ——
        (MatchKey::Single(value), Kind::Time) => {
            let time = pacs_core::query::parse_tm(value)?;
            let placeholder = next(binds);
            binds.push(Bind::Time(time));
            Some(format!("{} = {placeholder}", column.filter))
        }
        (MatchKey::TimeRange { from, to }, Kind::Time) => {
            range_condition(column.filter, *from, *to, binds, Bind::Time)
        }

        // —— 整数 ——
        (MatchKey::Single(value), Kind::Int) => {
            // IS VR 允许前后补空格,也允许带正号
            let parsed: i32 = value.trim().trim_start_matches('+').parse().ok()?;
            let placeholder = next(binds);
            binds.push(Bind::Int(parsed));
            Some(format!("{} = {placeholder}", column.filter))
        }

        // 其余组合(如对整数列用通配符、对日期列用 UID 列表)没有标准语义
        _ => None,
    }
}

fn range_condition<T: Copy>(
    filter: &str,
    from: Option<T>,
    to: Option<T>,
    binds: &mut Vec<Bind>,
    wrap: fn(T) -> Bind,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(value) = from {
        binds.push(wrap(value));
        parts.push(format!("{filter} >= ${}", binds.len()));
    }
    if let Some(value) = to {
        binds.push(wrap(value));
        parts.push(format!("{filter} <= ${}", binds.len()));
    }
    (!parts.is_empty()).then(|| parts.join(" AND "))
}

/// PN 的匹配值要和库里存的规范化形式走同一套变换,否则永远匹配不上。
fn normalize_for(column: &Column, value: &str) -> String {
    if column.vr == VR::PN && column.filter.ends_with("name_normalized") {
        pacs_core::normalize_person_name(value)
    } else {
        value.to_owned()
    }
}

/// 把一行结果翻成 C-FIND-RSP 的标识符数据集。
///
/// 值为 NULL 的键回零长元素而不是省略:标准里这些是 Type 2,
/// 「查到了但这个字段没有值」和「没查这个字段」是两回事。
fn build_identifier(
    row: &sqlx::postgres::PgRow,
    selected: &[&Column],
    level: QueryLevel,
) -> Result<InMemDicomObject, DbError> {
    let mut elements = vec![
        // 库里存的是 UTF-8,写出去的字节也是 UTF-8,必须明说 ——
        // 不带这一项时接收方按默认字符集(ASCII)解,中文姓名会变成乱码。
        // ISO_IR 192 就是 UTF-8 的 DICOM 定义术语(PS3.3 C.12.1.1.2)。
        // 纯 ASCII 的响应带上它也无害:ASCII 是 UTF-8 的子集。
        DataElement::new(
            tags::SPECIFIC_CHARACTER_SET,
            VR::CS,
            PrimitiveValue::from("ISO_IR 192"),
        ),
        DataElement::new(
            tags::QUERY_RETRIEVE_LEVEL,
            VR::CS,
            PrimitiveValue::from(level.as_str()),
        ),
    ];

    for (index, column) in selected.iter().enumerate() {
        let value = match column.kind {
            Kind::Text => row
                .try_get::<Option<String>, _>(index)?
                .map(PrimitiveValue::from),
            Kind::TextArray => row
                .try_get::<Option<Vec<String>>, _>(index)?
                .filter(|values| !values.is_empty())
                .map(|values| PrimitiveValue::Strs(values.into())),
            Kind::Date => row
                .try_get::<Option<NaiveDate>, _>(index)?
                .map(|date| PrimitiveValue::from(date.format("%Y%m%d").to_string())),
            Kind::Time => row.try_get::<Option<NaiveTime>, _>(index)?.map(|time| {
                use chrono::Timelike;
                // 秒以下全零时不写小数点 —— 多数设备的解析器对 `.000000` 宽容,
                // 但不写更干净,也和大多数 PACS 的输出一致。
                let formatted = if time.nanosecond() == 0 {
                    time.format("%H%M%S").to_string()
                } else {
                    time.format("%H%M%S%.6f").to_string()
                };
                PrimitiveValue::from(formatted)
            }),
            Kind::Int => row.try_get::<Option<i32>, _>(index)?.map(|number| {
                // US 是二进制整数,IS 是十进制字符串 —— 混淆会让对方解出乱码
                if column.vr == VR::US {
                    PrimitiveValue::from(u16::try_from(number).unwrap_or(u16::MAX))
                } else {
                    PrimitiveValue::from(number.to_string())
                }
            }),
        };

        elements.push(DataElement::new(
            column.tag,
            column.vr,
            value.unwrap_or(PrimitiveValue::Empty),
        ));
    }

    // 数据集元素必须按标签升序
    elements.sort_by_key(|element| element.header().tag);
    Ok(InMemDicomObject::from_element_iter(elements))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_are_scoped_to_their_level() {
        // Modality 属于 Series 层,PATIENT/STUDY 层查询用不了
        assert!(column_for(tags::MODALITY, QueryLevel::Patient).is_none());
        assert!(column_for(tags::MODALITY, QueryLevel::Study).is_none());
        assert!(column_for(tags::MODALITY, QueryLevel::Series).is_some());
        assert!(column_for(tags::MODALITY, QueryLevel::Image).is_some());

        // 病人属性在每一层都可用 —— patients 表始终在 JOIN 里
        for level in [
            QueryLevel::Patient,
            QueryLevel::Study,
            QueryLevel::Series,
            QueryLevel::Image,
        ] {
            assert!(column_for(tags::PATIENT_NAME, level).is_some());
        }
    }

    #[test]
    fn no_duplicate_tags_in_the_column_table() {
        let mut seen = std::collections::HashSet::new();
        for column in COLUMNS {
            assert!(
                seen.insert(column.tag),
                "{:?} 在列表里出现了两次,column_for 会取到不确定的那一个",
                column.tag
            );
        }
    }

    #[test]
    fn placeholders_are_numbered_consecutively() {
        let column = column_for(tags::STUDY_DATE, QueryLevel::Study).unwrap();
        let mut binds = Vec::new();

        let first = build_condition(
            column_for(tags::PATIENT_ID, QueryLevel::Study).unwrap(),
            &MatchKey::Single("123".into()),
            &mut binds,
        )
        .unwrap();
        assert_eq!(first, "p.patient_id = $1");

        let second = build_condition(
            column,
            &MatchKey::DateRange {
                from: NaiveDate::from_ymd_opt(2024, 1, 1),
                to: NaiveDate::from_ymd_opt(2024, 1, 31),
            },
            &mut binds,
        )
        .unwrap();
        assert_eq!(second, "s.study_date >= $2 AND s.study_date <= $3");
        assert_eq!(binds.len(), 3);
    }

    #[test]
    fn person_name_matching_uses_the_normalized_column() {
        let column = column_for(tags::PATIENT_NAME, QueryLevel::Study).unwrap();
        assert_eq!(column.select, "p.name", "返回的必须是原始姓名");

        let mut binds = Vec::new();
        let condition =
            build_condition(column, &MatchKey::Single("Zhang^San".into()), &mut binds).unwrap();
        assert_eq!(condition, "p.name_normalized = $1");
        // 查询值走了同一套规范化
        assert!(matches!(&binds[0], Bind::Text(value) if value == "ZHANG^SAN"));
    }

    #[test]
    fn wildcard_condition_declares_its_escape_character() {
        let column = column_for(tags::PATIENT_NAME, QueryLevel::Study).unwrap();
        let mut binds = Vec::new();
        let condition =
            build_condition(column, &MatchKey::Wildcard("Zhang*".into()), &mut binds).unwrap();
        assert_eq!(condition, r"p.name_normalized LIKE $1 ESCAPE '\'");
        assert!(matches!(&binds[0], Bind::Text(value) if value == "ZHANG%"));
    }

    /// 对整数列用通配符没有标准语义,必须报「不支持」而不是拼出错误的条件。
    #[test]
    fn mismatched_match_type_yields_no_condition() {
        let column = column_for(tags::SERIES_NUMBER, QueryLevel::Series).unwrap();
        let mut binds = Vec::new();
        assert!(build_condition(column, &MatchKey::Wildcard("1*".into()), &mut binds).is_none());
        assert!(binds.is_empty(), "失败的条件不该留下悬空参数");

        // 非数字的单值同样拒绝
        assert!(build_condition(column, &MatchKey::Single("abc".into()), &mut binds).is_none());
        assert!(binds.is_empty());
    }

    #[test]
    fn modalities_in_study_matches_by_containment() {
        let column = column_for(tags::MODALITIES_IN_STUDY, QueryLevel::Study).unwrap();
        let mut binds = Vec::new();
        let condition =
            build_condition(column, &MatchKey::Single("CT".into()), &mut binds).unwrap();
        assert_eq!(condition, "$1 = ANY(s.modalities)");
    }
}
