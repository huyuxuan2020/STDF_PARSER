use compact_str::{format_compact, CompactString};
use serde::{Deserialize, Deserializer, Serialize};
use std::borrow::Cow;
use std::io::{self, Read};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ParseProgress {
    pub session_id: String,
    pub bytes_read: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ParseErrorEvent {
    pub session_id: String,
    pub message: String,
    pub offset: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ParsedRecord {
    /// Static name from the REC_TYP/REC_SUB table — `&'static str` on purpose:
    /// a per-record `String` was one of the top allocations on 26M-record files.
    pub record_type: &'static str,
    /// Raw STDF REC_TYP/REC_SUB pair — identifies the static field spec this
    /// record was parsed with (used by the values-only field storage).
    pub rec_typ: u8,
    pub rec_sub: u8,
    pub offset: u64,
    pub length: u16,
    pub fields: Vec<ParsedField>,
    pub status: RecordStatus,
    /// Full RTN_RSLT array for MPR records. The generic field pipeline caps
    /// array previews at 16 elements to bound the session index, but the
    /// test-item accumulator must judge P/F and summarize over EVERY pin, so
    /// MPR records carry the raw floats out-of-band. In-process only — never
    /// serialized or stored.
    #[serde(skip)]
    pub mpr_results: Option<Vec<f32>>,
    /// Structural problem found while decoding this record. Kept out of the
    /// stored/serialized record shape; the session-level diagnostics collector
    /// turns it into a bounded, human-readable file issue summary.
    #[serde(skip)]
    pub parse_issue: Option<RecordParseIssue>,
    /// A bounded payload prefix retained only for UNKNOWN records. Diagnostics
    /// use it to prove a nearby record boundary without changing parse flow.
    #[serde(skip)]
    pub diagnostic_payload_prefix: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordParseIssue {
    pub kind: RecordParseIssueKind,
    pub field_name: &'static str,
    pub expected_bytes: usize,
    pub remaining_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordParseIssueKind {
    RequiredFieldMissing,
    FixedFieldTruncated,
    DeclaredLengthExceedsRecord,
    ArrayExceedsRecord,
    UnexpectedTrailingBytes,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ParsedField {
    pub name: Cow<'static, str>,
    pub field_type: Cow<'static, str>,
    /// CompactString: values ≤24 bytes live inline. Most STDF field values are
    /// short numbers/flags, and a String each made the allocator the top parse
    /// cost (~20 fields per PTR × 26M records). Serializes exactly like String.
    pub value: CompactString,
    pub description: Cow<'static, str>,
    pub offset: Option<u64>,
    pub length: Option<u16>,
}

impl<'de> Deserialize<'de> for ParsedField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct ParsedFieldJson {
            name: String,
            field_type: String,
            value: String,
            description: String,
            offset: Option<u64>,
            length: Option<u16>,
        }

        let field = ParsedFieldJson::deserialize(deserializer)?;
        Ok(Self {
            name: Cow::Owned(field.name),
            field_type: Cow::Owned(field.field_type),
            value: field.value.into(),
            description: Cow::Owned(field.description),
            offset: field.offset,
            length: field.length,
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RecordStatus {
    Parsed,
    Unknown,
    Error,
}

#[derive(Debug, Error)]
pub enum ParserError {
    #[error("读取 STDF 失败: {0}")]
    Io(#[from] io::Error),
    #[error(
        "文件末尾的 record header 不完整: offset {offset}, 需要 4 字节，实际只有 {available} 字节"
    )]
    TruncatedHeader { offset: u64, available: usize },
    #[error(
        "record payload 不完整: offset {offset}, REC_TYP={rec_typ}, REC_SUB={rec_sub}, 声明需要 {expected} 字节，实际只有 {available} 字节"
    )]
    TruncatedPayload {
        offset: u64,
        expected: usize,
        available: usize,
        rec_typ: u8,
        rec_sub: u8,
    },
    #[error("文件使用了当前版本不支持的 big-endian STDF 字节序: offset {offset}")]
    UnsupportedByteOrder { offset: u64 },
}

pub fn parse_reader<R: Read>(
    reader: &mut R,
    total_bytes: u64,
    mut on_record: impl FnMut(ParsedRecord) -> bool,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<(), ParserError> {
    let mut offset = 0_u64;
    // One payload buffer reused across records — a fresh Vec per record made
    // the allocator a top cost on million-record files. Fields copy what they
    // keep into owned values, so nothing borrows past the iteration.
    let mut payload = Vec::<u8>::new();
    loop {
        let mut header = [0_u8; 4];
        let mut header_read = 0_usize;
        while header_read < header.len() {
            match reader.read(&mut header[header_read..]) {
                Ok(0) if header_read == 0 => return Ok(()),
                Ok(0) => {
                    return Err(ParserError::TruncatedHeader {
                        offset,
                        available: header_read,
                    });
                }
                Ok(read) => header_read += read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(ParserError::Io(error)),
            }
        }

        let length = u16::from_le_bytes([header[0], header[1]]);
        let rec_typ = header[2];
        let rec_sub = header[3];
        if offset == 0
            && (rec_typ, rec_sub) == (0, 10)
            && length != 2
            && u16::from_be_bytes([header[0], header[1]]) == 2
        {
            return Err(ParserError::UnsupportedByteOrder { offset });
        }
        let len = length as usize;
        if payload.len() < len {
            payload.resize(len, 0);
        }
        let payload_start = offset + 4;
        let mut payload_read = 0_usize;
        while payload_read < len {
            match reader.read(&mut payload[payload_read..len]) {
                Ok(0) => {
                    return Err(ParserError::TruncatedPayload {
                        offset,
                        expected: len,
                        available: payload_read,
                        rec_typ,
                        rec_sub,
                    });
                }
                Ok(read) => payload_read += read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(ParserError::Io(error)),
            }
        }

        let record = parse_record(
            rec_typ,
            rec_sub,
            offset,
            length,
            payload_start,
            &payload[..len],
        );
        offset += 4 + u64::from(length);
        if !on_record(record) {
            break;
        }
        on_progress(offset, total_bytes);
    }
    Ok(())
}

fn parse_record(
    rec_typ: u8,
    rec_sub: u8,
    offset: u64,
    length: u16,
    payload_start: u64,
    payload: &[u8],
) -> ParsedRecord {
    let record_type = record_name(rec_typ, rec_sub);
    let mut cursor = FieldCursor::new(payload_start, payload);
    let (fields, has_required_error, parse_issue) =
        if matches!((rec_typ, rec_sub), (50, 10) | (50, 30)) {
            (Vec::new(), false, None)
        } else if let Some(specs) = record_specs(rec_typ, rec_sub) {
            cursor.parse_specs(specs)
        } else {
            (
                raw_preview_field(payload_start, length, payload),
                false,
                None,
            )
        };
    let status = if has_required_error {
        RecordStatus::Error
    } else if record_type == "UNKNOWN" {
        RecordStatus::Unknown
    } else {
        RecordStatus::Parsed
    };
    let mpr_results = if (rec_typ, rec_sub) == (15, 15) {
        extract_mpr_results(payload_start, &fields, payload)
    } else {
        None
    };
    let diagnostic_payload_prefix = if record_type == "UNKNOWN" {
        payload.iter().take(128).copied().collect()
    } else {
        Vec::new()
    };
    ParsedRecord {
        record_type,
        rec_typ,
        rec_sub,
        offset,
        length,
        fields,
        status,
        mpr_results,
        parse_issue,
        diagnostic_payload_prefix,
    }
}

/// Decode the complete RTN_RSLT float array from an MPR payload, using the
/// parsed field's recorded offset/length to locate the bytes (the field VALUE
/// itself only carries the capped preview).
fn extract_mpr_results(
    payload_start: u64,
    fields: &[ParsedField],
    payload: &[u8],
) -> Option<Vec<f32>> {
    let field = fields.iter().find(|field| field.name == "RTN_RSLT")?;
    let length = field.length? as usize;
    if length == 0 {
        return None;
    }
    let start = field.offset?.checked_sub(payload_start)? as usize;
    let bytes = payload.get(start..start.checked_add(length)?)?;
    Some(
        bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect(),
    )
}

fn raw_preview_field(payload_start: u64, length: u16, payload: &[u8]) -> Vec<ParsedField> {
    vec![ParsedField {
        name: Cow::Borrowed("RAW_BYTES"),
        field_type: Cow::Borrowed("BLOB"),
        value: hex_preview(payload).into(),
        description: Cow::Borrowed("默认不展开的 record 原始 payload 预览"),
        offset: Some(payload_start),
        length: Some(length),
    }]
}

#[derive(Clone, Copy)]
pub(crate) struct FieldSpec {
    pub(crate) name: &'static str,
    pub(crate) field_type: &'static str,
    pub(crate) description: &'static str,
    kind: FieldKind,
    required: bool,
}

#[derive(Clone, Copy)]
enum FieldKind {
    U1,
    U2,
    U4,
    I1,
    I2,
    I4,
    R4,
    C1,
    B1,
    Cn,
    Bn,
    Dn,
    ArrayU1(&'static str),
    ArrayU2(&'static str),
    ArrayR4(&'static str),
    ArrayCn(&'static str),
    ArrayN1(&'static str),
}

macro_rules! field {
    ($name:literal, $ty:literal, $desc:literal, $kind:expr) => {
        FieldSpec {
            name: $name,
            field_type: $ty,
            description: $desc,
            kind: $kind,
            required: true,
        }
    };
    (? $name:literal, $ty:literal, $desc:literal, $kind:expr) => {
        FieldSpec {
            name: $name,
            field_type: $ty,
            description: $desc,
            kind: $kind,
            required: false,
        }
    };
}

static FAR_FIELDS: &[FieldSpec] = &[
    field!("CPU_TYPE", "U*1", "CPU 类型", FieldKind::U1),
    field!("STDF_VER", "U*1", "STDF 版本", FieldKind::U1),
];

static ATR_FIELDS: &[FieldSpec] = &[
    field!("MOD_TIM", "U*4", "STDF 文件修改时间", FieldKind::U4),
    field!("CMD_LINE", "C*n", "执行命令行", FieldKind::Cn),
];

static MIR_FIELDS: &[FieldSpec] = &[
    field!("SETUP_T", "U*4", "测试程序 setup 时间", FieldKind::U4),
    field!("START_T", "U*4", "第一颗器件测试开始时间", FieldKind::U4),
    field!("STAT_NUM", "U*1", "测试站编号", FieldKind::U1),
    field!("MODE_COD", "C*1", "测试模式代码", FieldKind::C1),
    field!("RTST_COD", "C*1", "批次重测代码", FieldKind::C1),
    field!("PROT_COD", "C*1", "数据保护代码", FieldKind::C1),
    field!("BURN_TIM", "U*2", "老化时间分钟", FieldKind::U2),
    field!("CMOD_COD", "C*1", "命令模式代码", FieldKind::C1),
    field!("LOT_ID", "C*n", "批次编号", FieldKind::Cn),
    field!("PART_TYP", "C*n", "产品类型", FieldKind::Cn),
    field!("NODE_NAM", "C*n", "数据生成节点名称", FieldKind::Cn),
    field!("TSTR_TYP", "C*n", "测试机类型", FieldKind::Cn),
    field!("JOB_NAM", "C*n", "测试程序名称", FieldKind::Cn),
    field!(? "JOB_REV", "C*n", "测试程序版本", FieldKind::Cn),
    field!(? "SBLOT_ID", "C*n", "子批次编号", FieldKind::Cn),
    field!(? "OPER_NAM", "C*n", "操作员", FieldKind::Cn),
    field!(? "EXEC_TYP", "C*n", "执行软件类型", FieldKind::Cn),
    field!(? "EXEC_VER", "C*n", "执行软件版本", FieldKind::Cn),
    field!(? "TEST_COD", "C*n", "测试阶段代码", FieldKind::Cn),
    field!(? "TST_TEMP", "C*n", "测试温度", FieldKind::Cn),
    field!(? "USER_TXT", "C*n", "用户文本", FieldKind::Cn),
    field!(? "AUX_FILE", "C*n", "辅助文件", FieldKind::Cn),
    field!(? "PKG_TYP", "C*n", "封装类型", FieldKind::Cn),
    field!(? "FAMLY_ID", "C*n", "产品族编号", FieldKind::Cn),
    field!(? "DATE_COD", "C*n", "日期代码", FieldKind::Cn),
    field!(? "FACIL_ID", "C*n", "测试厂区编号", FieldKind::Cn),
    field!(? "FLOOR_ID", "C*n", "测试楼层编号", FieldKind::Cn),
    field!(? "PROC_ID", "C*n", "制程编号", FieldKind::Cn),
    field!(? "OPER_FRQ", "C*n", "操作频率或步骤", FieldKind::Cn),
    field!(? "SPEC_NAM", "C*n", "测试规格名称", FieldKind::Cn),
    field!(? "SPEC_VER", "C*n", "测试规格版本", FieldKind::Cn),
    field!(? "FLOW_ID", "C*n", "测试流程编号", FieldKind::Cn),
    field!(? "SETUP_ID", "C*n", "测试 setup 编号", FieldKind::Cn),
    field!(? "DSGN_REV", "C*n", "设计版本", FieldKind::Cn),
    field!(? "ENG_ID", "C*n", "工程批编号", FieldKind::Cn),
    field!(? "ROM_COD", "C*n", "ROM 代码编号", FieldKind::Cn),
    field!(? "SERL_NUM", "C*n", "测试机序列号", FieldKind::Cn),
    field!(? "SUPR_NAM", "C*n", "主管名称或编号", FieldKind::Cn),
];

static MRR_FIELDS: &[FieldSpec] = &[
    field!("FINISH_T", "U*4", "最后器件测试时间", FieldKind::U4),
    field!(? "DISP_COD", "C*1", "批次处置代码", FieldKind::C1),
    field!(? "USR_DESC", "C*n", "用户批次说明", FieldKind::Cn),
    field!(? "EXC_DESC", "C*n", "执行软件批次说明", FieldKind::Cn),
];

static PCR_FIELDS: &[FieldSpec] = &[
    field!("HEAD_NUM", "U*1", "测试头编号", FieldKind::U1),
    field!("SITE_NUM", "U*1", "测试站点编号", FieldKind::U1),
    field!("PART_CNT", "U*4", "测试器件数量", FieldKind::U4),
    field!(? "RTST_CNT", "U*4", "重测器件数量", FieldKind::U4),
    field!(? "ABRT_CNT", "U*4", "测试中止数量", FieldKind::U4),
    field!(? "GOOD_CNT", "U*4", "良品数量", FieldKind::U4),
    field!(? "FUNC_CNT", "U*4", "功能可测器件数量", FieldKind::U4),
];

static HBR_FIELDS: &[FieldSpec] = &[
    field!("HEAD_NUM", "U*1", "测试头编号", FieldKind::U1),
    field!("SITE_NUM", "U*1", "测试站点编号", FieldKind::U1),
    field!("HBIN_NUM", "U*2", "硬 bin 编号", FieldKind::U2),
    field!("HBIN_CNT", "U*4", "硬 bin 数量", FieldKind::U4),
    field!(? "HBIN_PF", "C*1", "硬 bin 通过/失败标记", FieldKind::C1),
    field!(? "HBIN_NAM", "C*n", "硬 bin 名称", FieldKind::Cn),
];

static SBR_FIELDS: &[FieldSpec] = &[
    field!("HEAD_NUM", "U*1", "测试头编号", FieldKind::U1),
    field!("SITE_NUM", "U*1", "测试站点编号", FieldKind::U1),
    field!("SBIN_NUM", "U*2", "软 bin 编号", FieldKind::U2),
    field!("SBIN_CNT", "U*4", "软 bin 数量", FieldKind::U4),
    field!(? "SBIN_PF", "C*1", "软 bin 通过/失败标记", FieldKind::C1),
    field!(? "SBIN_NAM", "C*n", "软 bin 名称", FieldKind::Cn),
];

static PMR_FIELDS: &[FieldSpec] = &[
    field!("PMR_INDX", "U*2", "pin 唯一索引", FieldKind::U2),
    field!(? "CHAN_TYP", "U*2", "通道类型", FieldKind::U2),
    field!(? "CHAN_NAM", "C*n", "通道名称", FieldKind::Cn),
    field!(? "PHY_NAM", "C*n", "物理 pin 名称", FieldKind::Cn),
    field!(? "LOG_NAM", "C*n", "逻辑 pin 名称", FieldKind::Cn),
    field!(? "HEAD_NUM", "U*1", "测试头编号", FieldKind::U1),
    field!(? "SITE_NUM", "U*1", "测试站点编号", FieldKind::U1),
];

static PGR_FIELDS: &[FieldSpec] = &[
    field!("GRP_INDX", "U*2", "pin group 唯一索引", FieldKind::U2),
    field!("GRP_NAM", "C*n", "pin group 名称", FieldKind::Cn),
    field!("INDX_CNT", "U*2", "PMR 索引数量", FieldKind::U2),
    field!(? "PMR_INDX", "kxU*2", "pin group 内 PMR 索引数组", FieldKind::ArrayU2("INDX_CNT")),
];

static PLR_FIELDS: &[FieldSpec] = &[
    field!("GRP_CNT", "U*2", "pin 或 pin group 数量", FieldKind::U2),
    field!(
        "GRP_INDX",
        "kxU*2",
        "pin 或 pin group 索引数组",
        FieldKind::ArrayU2("GRP_CNT")
    ),
    field!(? "GRP_MODE", "kxU*2", "pin group 操作模式数组", FieldKind::ArrayU2("GRP_CNT")),
    field!(? "GRP_RADX", "kxU*1", "pin group 显示 radix 数组", FieldKind::ArrayU1("GRP_CNT")),
    field!(? "PGM_CHAR", "kxC*n", "program state 字符数组", FieldKind::ArrayCn("GRP_CNT")),
    field!(? "RTN_CHAR", "kxC*n", "return state 字符数组", FieldKind::ArrayCn("GRP_CNT")),
    field!(? "PGM_CHAL", "kxC*n", "program state 第二字符数组", FieldKind::ArrayCn("GRP_CNT")),
    field!(? "RTN_CHAL", "kxC*n", "return state 第二字符数组", FieldKind::ArrayCn("GRP_CNT")),
];

static RDR_FIELDS: &[FieldSpec] = &[
    field!("NUM_BINS", "U*2", "重测 bin 数量", FieldKind::U2),
    field!(? "RTST_BIN", "kxU*2", "重测 bin 编号数组", FieldKind::ArrayU2("NUM_BINS")),
];

static SDR_FIELDS: &[FieldSpec] = &[
    field!("HEAD_NUM", "U*1", "测试头编号", FieldKind::U1),
    field!("SITE_GRP", "U*1", "站点组编号", FieldKind::U1),
    field!("SITE_CNT", "U*1", "站点数量", FieldKind::U1),
    field!(
        "SITE_NUM",
        "kxU*1",
        "测试站点编号数组",
        FieldKind::ArrayU1("SITE_CNT")
    ),
    field!(? "HAND_TYP", "C*n", "handler/prober 类型", FieldKind::Cn),
    field!(? "HAND_ID", "C*n", "handler/prober 编号", FieldKind::Cn),
    field!(? "CARD_TYP", "C*n", "probe card 类型", FieldKind::Cn),
    field!(? "CARD_ID", "C*n", "probe card 编号", FieldKind::Cn),
    field!(? "LOAD_TYP", "C*n", "load board 类型", FieldKind::Cn),
    field!(? "LOAD_ID", "C*n", "load board 编号", FieldKind::Cn),
    field!(? "DIB_TYP", "C*n", "DIB board 类型", FieldKind::Cn),
    field!(? "DIB_ID", "C*n", "DIB board 编号", FieldKind::Cn),
    field!(? "CABL_TYP", "C*n", "接口线缆类型", FieldKind::Cn),
    field!(? "CABL_ID", "C*n", "接口线缆编号", FieldKind::Cn),
    field!(? "CONT_TYP", "C*n", "接触器类型", FieldKind::Cn),
    field!(? "CONT_ID", "C*n", "接触器编号", FieldKind::Cn),
    field!(? "LASR_TYP", "C*n", "laser 类型", FieldKind::Cn),
    field!(? "LASR_ID", "C*n", "laser 编号", FieldKind::Cn),
    field!(? "EXTR_TYP", "C*n", "额外设备类型", FieldKind::Cn),
    field!(? "EXTR_ID", "C*n", "额外设备编号", FieldKind::Cn),
];

static WIR_FIELDS: &[FieldSpec] = &[
    field!("HEAD_NUM", "U*1", "测试头编号", FieldKind::U1),
    field!("SITE_GRP", "U*1", "站点组编号", FieldKind::U1),
    field!("START_T", "U*4", "晶圆测试开始时间", FieldKind::U4),
    field!(? "WAFER_ID", "C*n", "晶圆编号", FieldKind::Cn),
];

static WRR_FIELDS: &[FieldSpec] = &[
    field!("HEAD_NUM", "U*1", "测试头编号", FieldKind::U1),
    field!("SITE_GRP", "U*1", "站点组编号", FieldKind::U1),
    field!("FINISH_T", "U*4", "晶圆测试结束时间", FieldKind::U4),
    field!("PART_CNT", "U*4", "测试器件数量", FieldKind::U4),
    field!(? "RTST_CNT", "U*4", "重测器件数量", FieldKind::U4),
    field!(? "ABRT_CNT", "U*4", "测试中止数量", FieldKind::U4),
    field!(? "GOOD_CNT", "U*4", "良品数量", FieldKind::U4),
    field!(? "FUNC_CNT", "U*4", "功能可测器件数量", FieldKind::U4),
    field!(? "WAFER_ID", "C*n", "晶圆编号", FieldKind::Cn),
    field!(? "FABWF_ID", "C*n", "fab 晶圆编号", FieldKind::Cn),
    field!(? "FRAME_ID", "C*n", "晶圆框编号", FieldKind::Cn),
    field!(? "MASK_ID", "C*n", "mask 编号", FieldKind::Cn),
    field!(? "USR_DESC", "C*n", "用户晶圆说明", FieldKind::Cn),
    field!(? "EXC_DESC", "C*n", "执行软件晶圆说明", FieldKind::Cn),
];

static WCR_FIELDS: &[FieldSpec] = &[
    field!("WAFR_SIZ", "R*4", "晶圆直径", FieldKind::R4),
    field!("DIE_HT", "R*4", "die 高度", FieldKind::R4),
    field!("DIE_WID", "R*4", "die 宽度", FieldKind::R4),
    field!("WF_UNITS", "U*1", "晶圆和 die 尺寸单位", FieldKind::U1),
    field!("WF_FLAT", "C*1", "晶圆 flat 方向", FieldKind::C1),
    field!("CENTER_X", "I*2", "中心 die X 坐标", FieldKind::I2),
    field!("CENTER_Y", "I*2", "中心 die Y 坐标", FieldKind::I2),
    field!("POS_X", "C*1", "晶圆正 X 方向", FieldKind::C1),
    field!("POS_Y", "C*1", "晶圆正 Y 方向", FieldKind::C1),
];

static PIR_FIELDS: &[FieldSpec] = &[
    field!("HEAD_NUM", "U*1", "测试头编号", FieldKind::U1),
    field!("SITE_NUM", "U*1", "测试站点编号", FieldKind::U1),
];

static PRR_FIELDS: &[FieldSpec] = &[
    field!("HEAD_NUM", "U*1", "测试头编号", FieldKind::U1),
    field!("SITE_NUM", "U*1", "测试站点编号", FieldKind::U1),
    field!("PART_FLG", "B*1", "器件信息标志", FieldKind::B1),
    field!("NUM_TEST", "U*2", "执行测试数量", FieldKind::U2),
    field!("HARD_BIN", "U*2", "硬 bin 编号", FieldKind::U2),
    field!(? "SOFT_BIN", "U*2", "软 bin 编号", FieldKind::U2),
    field!(? "X_COORD", "I*2", "晶圆 X 坐标", FieldKind::I2),
    field!(? "Y_COORD", "I*2", "晶圆 Y 坐标", FieldKind::I2),
    field!(? "TEST_T", "U*4", "测试耗时毫秒", FieldKind::U4),
    field!(? "PART_ID", "C*n", "器件编号", FieldKind::Cn),
    field!(? "PART_TXT", "C*n", "器件说明文本", FieldKind::Cn),
    field!(? "PART_FIX", "B*n", "器件修复信息", FieldKind::Bn),
];

static TSR_FIELDS: &[FieldSpec] = &[
    field!("HEAD_NUM", "U*1", "测试头编号", FieldKind::U1),
    field!("SITE_NUM", "U*1", "测试站点编号", FieldKind::U1),
    field!("TEST_TYP", "C*1", "测试类型", FieldKind::C1),
    field!("TEST_NUM", "U*4", "测试编号", FieldKind::U4),
    field!(? "EXEC_CNT", "U*4", "测试执行次数", FieldKind::U4),
    field!(? "FAIL_CNT", "U*4", "测试失败次数", FieldKind::U4),
    field!(? "ALRM_CNT", "U*4", "测试报警次数", FieldKind::U4),
    field!(? "TEST_NAM", "C*n", "测试名称", FieldKind::Cn),
    field!(? "SEQ_NAME", "C*n", "程序段名称", FieldKind::Cn),
    field!(? "TEST_LBL", "C*n", "测试标签文本", FieldKind::Cn),
    field!(? "OPT_FLAG", "B*1", "可选数据标志", FieldKind::B1),
    field!(? "TEST_TIM", "R*4", "平均测试执行时间", FieldKind::R4),
    field!(? "TEST_MIN", "R*4", "最低测试结果值", FieldKind::R4),
    field!(? "TEST_MAX", "R*4", "最高测试结果值", FieldKind::R4),
    field!(? "TST_SUMS", "R*4", "测试结果总和", FieldKind::R4),
    field!(? "TST_SQRS", "R*4", "测试结果平方和", FieldKind::R4),
];

static PTR_FIELDS: &[FieldSpec] = &[
    field!("TEST_NUM", "U*4", "测试编号", FieldKind::U4),
    field!("HEAD_NUM", "U*1", "测试头编号", FieldKind::U1),
    field!("SITE_NUM", "U*1", "测试站点编号", FieldKind::U1),
    field!("TEST_FLG", "B*1", "测试标志", FieldKind::B1),
    field!("PARM_FLG", "B*1", "参数测试标志", FieldKind::B1),
    field!("RESULT", "R*4", "测试结果", FieldKind::R4),
    field!(? "TEST_TXT", "C*n", "测试说明文本或标签", FieldKind::Cn),
    field!(? "ALARM_ID", "C*n", "报警名称", FieldKind::Cn),
    field!(? "OPT_FLAG", "B*1", "可选数据标志", FieldKind::B1),
    field!(? "RES_SCAL", "I*1", "测试结果缩放指数", FieldKind::I1),
    field!(? "LLM_SCAL", "I*1", "低限缩放指数", FieldKind::I1),
    field!(? "HLM_SCAL", "I*1", "高限缩放指数", FieldKind::I1),
    field!(? "LO_LIMIT", "R*4", "低测试限", FieldKind::R4),
    field!(? "HI_LIMIT", "R*4", "高测试限", FieldKind::R4),
    field!(? "UNITS", "C*n", "测试单位", FieldKind::Cn),
    field!(? "C_RESFMT", "C*n", "结果格式字符串", FieldKind::Cn),
    field!(? "C_LLMFMT", "C*n", "低限格式字符串", FieldKind::Cn),
    field!(? "C_HLMFMT", "C*n", "高限格式字符串", FieldKind::Cn),
    field!(? "LO_SPEC", "R*4", "低规格限", FieldKind::R4),
    field!(? "HI_SPEC", "R*4", "高规格限", FieldKind::R4),
];

static MPR_FIELDS: &[FieldSpec] = &[
    field!("TEST_NUM", "U*4", "测试编号", FieldKind::U4),
    field!("HEAD_NUM", "U*1", "测试头编号", FieldKind::U1),
    field!("SITE_NUM", "U*1", "测试站点编号", FieldKind::U1),
    field!("TEST_FLG", "B*1", "测试标志", FieldKind::B1),
    field!("PARM_FLG", "B*1", "参数测试标志", FieldKind::B1),
    field!("RTN_ICNT", "U*2", "返回 PMR 索引数量", FieldKind::U2),
    field!("RSLT_CNT", "U*2", "返回结果数量", FieldKind::U2),
    field!(? "RTN_STAT", "jxN*1", "返回状态数组", FieldKind::ArrayN1("RTN_ICNT")),
    field!(? "RTN_RSLT", "kxR*4", "返回结果数组", FieldKind::ArrayR4("RSLT_CNT")),
    field!(? "TEST_TXT", "C*n", "测试说明文本或标签", FieldKind::Cn),
    field!(? "ALARM_ID", "C*n", "报警名称", FieldKind::Cn),
    field!(? "OPT_FLAG", "B*1", "可选数据标志", FieldKind::B1),
    field!(? "RES_SCAL", "I*1", "测试结果缩放指数", FieldKind::I1),
    field!(? "LLM_SCAL", "I*1", "低限缩放指数", FieldKind::I1),
    field!(? "HLM_SCAL", "I*1", "高限缩放指数", FieldKind::I1),
    field!(? "LO_LIMIT", "R*4", "低测试限", FieldKind::R4),
    field!(? "HI_LIMIT", "R*4", "高测试限", FieldKind::R4),
    field!(? "START_IN", "R*4", "输入条件起始值", FieldKind::R4),
    field!(? "INCR_IN", "R*4", "输入条件增量", FieldKind::R4),
    field!(? "RTN_INDX", "jxU*2", "PMR 索引数组", FieldKind::ArrayU2("RTN_ICNT")),
    field!(? "UNITS", "C*n", "返回结果单位", FieldKind::Cn),
    field!(? "UNITS_IN", "C*n", "输入条件单位", FieldKind::Cn),
    field!(? "C_RESFMT", "C*n", "结果格式字符串", FieldKind::Cn),
    field!(? "C_LLMFMT", "C*n", "低限格式字符串", FieldKind::Cn),
    field!(? "C_HLMFMT", "C*n", "高限格式字符串", FieldKind::Cn),
    field!(? "LO_SPEC", "R*4", "低规格限", FieldKind::R4),
    field!(? "HI_SPEC", "R*4", "高规格限", FieldKind::R4),
];

static FTR_FIELDS: &[FieldSpec] = &[
    field!("TEST_NUM", "U*4", "测试编号", FieldKind::U4),
    field!("HEAD_NUM", "U*1", "测试头编号", FieldKind::U1),
    field!("SITE_NUM", "U*1", "测试站点编号", FieldKind::U1),
    field!("TEST_FLG", "B*1", "测试标志", FieldKind::B1),
    field!("OPT_FLAG", "B*1", "可选数据标志", FieldKind::B1),
    field!(? "CYCL_CNT", "U*4", "vector cycle count", FieldKind::U4),
    field!(? "REL_VADR", "U*4", "相对 vector 地址", FieldKind::U4),
    field!(? "REPT_CNT", "U*4", "vector 重复次数", FieldKind::U4),
    field!(? "NUM_FAIL", "U*4", "失败 pin 数量", FieldKind::U4),
    field!(? "XFAIL_AD", "I*4", "X 逻辑失败地址", FieldKind::I4),
    field!(? "YFAIL_AD", "I*4", "Y 逻辑失败地址", FieldKind::I4),
    field!(? "VECT_OFF", "I*2", "目标 vector 偏移", FieldKind::I2),
    field!(? "RTN_ICNT", "U*2", "返回数据 PMR 索引数量", FieldKind::U2),
    field!(? "PGM_ICNT", "U*2", "programmed state 索引数量", FieldKind::U2),
    field!(? "RTN_INDX", "jxU*2", "返回数据 PMR 索引数组", FieldKind::ArrayU2("RTN_ICNT")),
    field!(? "RTN_STAT", "jxN*1", "返回状态数组", FieldKind::ArrayN1("RTN_ICNT")),
    field!(? "PGM_INDX", "kxU*2", "programmed state 索引数组", FieldKind::ArrayU2("PGM_ICNT")),
    field!(? "PGM_STAT", "kxN*1", "programmed state 数组", FieldKind::ArrayN1("PGM_ICNT")),
    field!(? "FAIL_PIN", "D*n", "失败 pin bitfield", FieldKind::Dn),
    field!(? "VECT_NAM", "C*n", "vector pattern 名称", FieldKind::Cn),
    field!(? "TIME_SET", "C*n", "time set 名称", FieldKind::Cn),
    field!(? "OP_CODE", "C*n", "vector op code", FieldKind::Cn),
    field!(? "TEST_TXT", "C*n", "测试说明文本", FieldKind::Cn),
    field!(? "ALARM_ID", "C*n", "报警名称", FieldKind::Cn),
    field!(? "PROG_TXT", "C*n", "programmed 附加信息", FieldKind::Cn),
    field!(? "RSLT_TXT", "C*n", "result 附加信息", FieldKind::Cn),
    field!(? "PATG_NUM", "U*1", "pattern generator 编号", FieldKind::U1),
    field!(? "SPIN_MAP", "D*n", "enabled comparator bit map", FieldKind::Dn),
];

static BPS_FIELDS: &[FieldSpec] = &[field!(? "SEQ_NAME", "C*n", "程序段名称", FieldKind::Cn)];
static EPS_FIELDS: &[FieldSpec] = &[];
pub(crate) fn record_specs(rec_typ: u8, rec_sub: u8) -> Option<&'static [FieldSpec]> {
    match (rec_typ, rec_sub) {
        (0, 10) => Some(FAR_FIELDS),
        (0, 20) => Some(ATR_FIELDS),
        (1, 10) => Some(MIR_FIELDS),
        (1, 20) => Some(MRR_FIELDS),
        (1, 30) => Some(PCR_FIELDS),
        (1, 40) => Some(HBR_FIELDS),
        (1, 50) => Some(SBR_FIELDS),
        (1, 60) => Some(PMR_FIELDS),
        (1, 62) => Some(PGR_FIELDS),
        (1, 63) => Some(PLR_FIELDS),
        (1, 70) => Some(RDR_FIELDS),
        (1, 80) => Some(SDR_FIELDS),
        (2, 10) => Some(WIR_FIELDS),
        (2, 20) => Some(WRR_FIELDS),
        (2, 30) => Some(WCR_FIELDS),
        (5, 10) => Some(PIR_FIELDS),
        (5, 20) => Some(PRR_FIELDS),
        (10, 30) => Some(TSR_FIELDS),
        (15, 10) => Some(PTR_FIELDS),
        (15, 15) => Some(MPR_FIELDS),
        (15, 20) => Some(FTR_FIELDS),
        (20, 10) => Some(BPS_FIELDS),
        (20, 20) => Some(EPS_FIELDS),
        _ => None,
    }
}

pub(crate) fn record_name(rec_typ: u8, rec_sub: u8) -> &'static str {
    match (rec_typ, rec_sub) {
        (0, 10) => "FAR",
        (0, 20) => "ATR",
        (1, 10) => "MIR",
        (1, 20) => "MRR",
        (1, 30) => "PCR",
        (1, 40) => "HBR",
        (1, 50) => "SBR",
        (1, 60) => "PMR",
        (1, 62) => "PGR",
        (1, 63) => "PLR",
        (1, 70) => "RDR",
        (1, 80) => "SDR",
        (2, 10) => "WIR",
        (2, 20) => "WRR",
        (2, 30) => "WCR",
        (5, 10) => "PIR",
        (5, 20) => "PRR",
        (10, 30) => "TSR",
        (15, 10) => "PTR",
        (15, 15) => "MPR",
        (15, 20) => "FTR",
        (20, 10) => "BPS",
        (20, 20) => "EPS",
        (50, 10) => "GDR",
        (50, 30) => "DTR",
        _ => "UNKNOWN",
    }
}

/// Decode a DTR TEXT_DAT payload (Cn: length byte + chars) with the same
/// tolerance as `FieldCursor::cn` — lossy UTF-8, length clamped to the payload.
pub fn decode_dtr_text(payload: &[u8]) -> String {
    let Some((&len, rest)) = payload.split_first() else {
        return String::new();
    };
    let end = (len as usize).min(rest.len());
    String::from_utf8_lossy(&rest[..end]).to_string()
}

/// Integer Display via itoa: byte-identical output to `to_string()` without
/// the fmt machinery, landing in an inline CompactString — zero heap traffic.
fn itoa_string(value: impl itoa::Integer) -> CompactString {
    CompactString::from(itoa::Buffer::new().format(value))
}

/// Lossy UTF-8 into a CompactString: the borrowed (valid-UTF-8) fast path
/// inlines short values without touching the heap.
fn lossy_compact(bytes: &[u8]) -> CompactString {
    match String::from_utf8_lossy(bytes) {
        Cow::Borrowed(text) => CompactString::from(text),
        Cow::Owned(text) => CompactString::from(text),
    }
}

fn hex_preview(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(32)
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

struct FieldCursor<'a> {
    payload_start: u64,
    cursor: usize,
    payload: &'a [u8],
    had_truncation: bool,
    first_issue: Option<RecordParseIssue>,
}

impl<'a> FieldCursor<'a> {
    fn new(payload_start: u64, payload: &'a [u8]) -> Self {
        Self {
            payload_start,
            cursor: 0,
            payload,
            had_truncation: false,
            first_issue: None,
        }
    }

    fn parse_specs(
        &mut self,
        specs: &[FieldSpec],
    ) -> (Vec<ParsedField>, bool, Option<RecordParseIssue>) {
        let mut values = Vec::<(&'static str, usize)>::new();
        let mut fields = Vec::with_capacity(specs.len());
        let mut has_required_error = false;
        for spec in specs {
            let field = self.read_spec(spec, &values);
            if spec.required && field.offset.is_none() {
                has_required_error = true;
                if self.first_issue.is_none() {
                    self.first_issue = Some(RecordParseIssue {
                        kind: RecordParseIssueKind::RequiredFieldMissing,
                        field_name: spec.name,
                        expected_bytes: minimum_field_bytes(spec.kind),
                        remaining_bytes: self.payload.len().saturating_sub(self.cursor),
                    });
                }
            }
            if let Some(value) = field.value.parse::<usize>().ok() {
                values.push((spec.name, value));
            }
            fields.push(field);
        }
        if self.first_issue.is_none() && self.cursor < self.payload.len() {
            self.first_issue = Some(RecordParseIssue {
                kind: RecordParseIssueKind::UnexpectedTrailingBytes,
                field_name: "RECORD_END",
                expected_bytes: 0,
                remaining_bytes: self.payload.len() - self.cursor,
            });
        }
        (
            fields,
            has_required_error || self.had_truncation,
            self.first_issue.clone(),
        )
    }

    fn mark_issue(
        &mut self,
        kind: RecordParseIssueKind,
        field_name: &'static str,
        expected_bytes: usize,
        remaining_bytes: usize,
    ) {
        if self.first_issue.is_none() {
            self.first_issue = Some(RecordParseIssue {
                kind,
                field_name,
                expected_bytes,
                remaining_bytes,
            });
        }
    }

    fn read_spec(&mut self, spec: &FieldSpec, values: &[(&'static str, usize)]) -> ParsedField {
        match spec.kind {
            FieldKind::U1 => self.u1(spec.name, spec.field_type, spec.description),
            FieldKind::U2 => self.u2(spec.name, spec.field_type, spec.description),
            FieldKind::U4 => self.u4(spec.name, spec.field_type, spec.description),
            FieldKind::I1 => self.i1(spec.name, spec.field_type, spec.description),
            FieldKind::I2 => self.i2(spec.name, spec.field_type, spec.description),
            FieldKind::I4 => self.i4(spec.name, spec.field_type, spec.description),
            FieldKind::R4 => self.r4(spec.name, spec.field_type, spec.description),
            FieldKind::C1 => self.c1(spec.name, spec.field_type, spec.description),
            FieldKind::B1 => self.b1(spec.name, spec.field_type, spec.description),
            FieldKind::Cn => self.cn(spec.name, spec.field_type, spec.description),
            FieldKind::Bn => self.bn(spec.name, spec.field_type, spec.description),
            FieldKind::Dn => self.dn(spec.name, spec.field_type, spec.description),
            FieldKind::ArrayU1(count) => self.array_fixed(
                spec.name,
                array_type(values, count, "U*1"),
                spec.description,
                count_value(values, count),
                1,
                |bytes| bytes[0].to_string(),
            ),
            FieldKind::ArrayU2(count) => self.array_fixed(
                spec.name,
                array_type(values, count, "U*2"),
                spec.description,
                count_value(values, count),
                2,
                |bytes| u16::from_le_bytes([bytes[0], bytes[1]]).to_string(),
            ),
            FieldKind::ArrayR4(count) => self.array_fixed(
                spec.name,
                array_type(values, count, "R*4"),
                spec.description,
                count_value(values, count),
                4,
                |bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]).to_string(),
            ),
            FieldKind::ArrayCn(count) => self.array_cn(
                spec.name,
                array_type(values, count, "C*n"),
                spec.description,
                count_value(values, count),
            ),
            FieldKind::ArrayN1(count) => self.array_n1(
                spec.name,
                array_type(values, count, "N*1"),
                spec.description,
                count_value(values, count),
            ),
        }
    }

    fn u1(
        &mut self,
        name: &'static str,
        field_type: &'static str,
        description: &'static str,
    ) -> ParsedField {
        self.fixed(name, field_type, description, 1, |bytes| {
            itoa_string(bytes[0])
        })
    }

    fn u2(
        &mut self,
        name: &'static str,
        field_type: &'static str,
        description: &'static str,
    ) -> ParsedField {
        self.fixed(name, field_type, description, 2, |bytes| {
            itoa_string(u16::from_le_bytes([bytes[0], bytes[1]]))
        })
    }

    fn u4(
        &mut self,
        name: &'static str,
        field_type: &'static str,
        description: &'static str,
    ) -> ParsedField {
        self.fixed(name, field_type, description, 4, |bytes| {
            itoa_string(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        })
    }

    fn i1(
        &mut self,
        name: &'static str,
        field_type: &'static str,
        description: &'static str,
    ) -> ParsedField {
        self.fixed(name, field_type, description, 1, |bytes| {
            itoa_string(bytes[0] as i8)
        })
    }

    fn i2(
        &mut self,
        name: &'static str,
        field_type: &'static str,
        description: &'static str,
    ) -> ParsedField {
        self.fixed(name, field_type, description, 2, |bytes| {
            itoa_string(i16::from_le_bytes([bytes[0], bytes[1]]))
        })
    }

    fn i4(
        &mut self,
        name: &'static str,
        field_type: &'static str,
        description: &'static str,
    ) -> ParsedField {
        self.fixed(name, field_type, description, 4, |bytes| {
            itoa_string(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        })
    }

    fn r4(
        &mut self,
        name: &'static str,
        field_type: &'static str,
        description: &'static str,
    ) -> ParsedField {
        self.fixed(name, field_type, description, 4, |bytes| {
            // fmt Display on purpose (not ryu): output must stay byte-identical
            // to the previous to_string() ("1", not "1.0"; no exponent form).
            format_compact!(
                "{}",
                f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
            )
        })
    }

    fn c1(
        &mut self,
        name: &'static str,
        field_type: &'static str,
        description: &'static str,
    ) -> ParsedField {
        self.fixed(name, field_type, description, 1, |bytes| {
            lossy_compact(bytes)
        })
    }

    fn b1(
        &mut self,
        name: &'static str,
        field_type: &'static str,
        description: &'static str,
    ) -> ParsedField {
        self.fixed(name, field_type, description, 1, |bytes| {
            format_compact!("0b{:08b}", bytes[0])
        })
    }

    fn cn(
        &mut self,
        name: &'static str,
        field_type: &'static str,
        description: &'static str,
    ) -> ParsedField {
        let start = self.cursor;
        if start >= self.payload.len() {
            return self.missing(name, field_type, description);
        }
        let len = self.payload[start] as usize;
        let value_start = start + 1;
        let expected_end = value_start.saturating_add(len);
        if expected_end > self.payload.len() {
            self.had_truncation = true;
            self.mark_issue(
                RecordParseIssueKind::DeclaredLengthExceedsRecord,
                name,
                len,
                self.payload.len().saturating_sub(value_start),
            );
        }
        let value_end = expected_end.min(self.payload.len());
        self.cursor = value_end;
        ParsedField {
            name: Cow::Borrowed(name),
            field_type: Cow::Borrowed(field_type),
            value: lossy_compact(&self.payload[value_start..value_end]),
            description: Cow::Borrowed(description),
            offset: Some(self.payload_start + start as u64),
            length: Some((value_end - start) as u16),
        }
    }

    fn bn(
        &mut self,
        name: &'static str,
        field_type: &'static str,
        description: &'static str,
    ) -> ParsedField {
        let start = self.cursor;
        if start >= self.payload.len() {
            return self.missing(name, field_type, description);
        }
        let len = self.payload[start] as usize;
        let value_start = start + 1;
        let expected_end = value_start.saturating_add(len);
        if expected_end > self.payload.len() {
            self.had_truncation = true;
            self.mark_issue(
                RecordParseIssueKind::DeclaredLengthExceedsRecord,
                name,
                len,
                self.payload.len().saturating_sub(value_start),
            );
        }
        let value_end = expected_end.min(self.payload.len());
        self.cursor = value_end;
        ParsedField {
            name: Cow::Borrowed(name),
            field_type: Cow::Borrowed(field_type),
            value: bytes_summary(&self.payload[value_start..value_end], len).into(),
            description: Cow::Borrowed(description),
            offset: Some(self.payload_start + start as u64),
            length: Some((value_end - start) as u16),
        }
    }

    fn dn(
        &mut self,
        name: &'static str,
        field_type: &'static str,
        description: &'static str,
    ) -> ParsedField {
        let start = self.cursor;
        if start + 2 > self.payload.len() {
            if start < self.payload.len() {
                self.had_truncation = true;
                self.mark_issue(
                    RecordParseIssueKind::FixedFieldTruncated,
                    name,
                    2,
                    self.payload.len() - start,
                );
            }
            self.cursor = self.payload.len();
            return self.missing(name, field_type, description);
        }
        let bit_count = u16::from_le_bytes([self.payload[start], self.payload[start + 1]]) as usize;
        let byte_count = bit_count.div_ceil(8);
        let value_start = start + 2;
        let expected_end = value_start.saturating_add(byte_count);
        if expected_end > self.payload.len() {
            self.had_truncation = true;
            self.mark_issue(
                RecordParseIssueKind::DeclaredLengthExceedsRecord,
                name,
                byte_count,
                self.payload.len().saturating_sub(value_start),
            );
        }
        let value_end = expected_end.min(self.payload.len());
        self.cursor = value_end;
        ParsedField {
            name: Cow::Borrowed(name),
            field_type: Cow::Borrowed(field_type),
            value: format_compact!(
                "bits={}, bytes={}, preview={}",
                bit_count,
                byte_count,
                hex_preview(&self.payload[value_start..value_end])
            ),
            description: Cow::Borrowed(description),
            offset: Some(self.payload_start + start as u64),
            length: Some((value_end - start) as u16),
        }
    }

    fn array_fixed(
        &mut self,
        name: &'static str,
        field_type: String,
        description: &'static str,
        count: usize,
        item_len: usize,
        read: impl Fn(&[u8]) -> String,
    ) -> ParsedField {
        let start = self.cursor;
        let total_len = count.saturating_mul(item_len);
        if total_len == 0 {
            return self.empty_array(name, field_type, description, start);
        }
        let end = start.saturating_add(total_len);
        if end > self.payload.len() {
            if start < self.payload.len() {
                self.had_truncation = true;
                self.mark_issue(
                    RecordParseIssueKind::ArrayExceedsRecord,
                    name,
                    total_len,
                    self.payload.len() - start,
                );
            }
            self.cursor = self.payload.len();
            return self.missing(name, field_type, description);
        }
        self.cursor = end;
        let values = self.payload[start..end]
            .chunks_exact(item_len)
            .take(16)
            .map(read)
            .collect::<Vec<_>>();
        ParsedField {
            name: Cow::Borrowed(name),
            field_type: Cow::Owned(field_type),
            value: array_summary(count, values).into(),
            description: Cow::Borrowed(description),
            offset: Some(self.payload_start + start as u64),
            length: Some(total_len as u16),
        }
    }

    fn array_cn(
        &mut self,
        name: &'static str,
        field_type: String,
        description: &'static str,
        count: usize,
    ) -> ParsedField {
        let start = self.cursor;
        if count == 0 {
            return self.empty_array(name, field_type, description, start);
        }
        let mut values = Vec::new();
        for _ in 0..count {
            if self.cursor >= self.payload.len() {
                self.cursor = self.payload.len();
                return self.missing(name, field_type, description);
            }
            let len = self.payload[self.cursor] as usize;
            let value_start = self.cursor + 1;
            let expected_end = value_start.saturating_add(len);
            if expected_end > self.payload.len() {
                self.had_truncation = true;
                self.mark_issue(
                    RecordParseIssueKind::ArrayExceedsRecord,
                    name,
                    len,
                    self.payload.len().saturating_sub(value_start),
                );
            }
            let value_end = expected_end.min(self.payload.len());
            values.push(String::from_utf8_lossy(&self.payload[value_start..value_end]).to_string());
            self.cursor = value_end;
        }
        ParsedField {
            name: Cow::Borrowed(name),
            field_type: Cow::Owned(field_type),
            value: array_summary(count, values.into_iter().take(16).collect()).into(),
            description: Cow::Borrowed(description),
            offset: Some(self.payload_start + start as u64),
            length: Some((self.cursor - start) as u16),
        }
    }

    fn array_n1(
        &mut self,
        name: &'static str,
        field_type: String,
        description: &'static str,
        count: usize,
    ) -> ParsedField {
        let start = self.cursor;
        if count == 0 {
            return self.empty_array(name, field_type, description, start);
        }
        let byte_count = count.div_ceil(2);
        let end = start.saturating_add(byte_count);
        if end > self.payload.len() {
            if start < self.payload.len() {
                self.had_truncation = true;
                self.mark_issue(
                    RecordParseIssueKind::ArrayExceedsRecord,
                    name,
                    byte_count,
                    self.payload.len() - start,
                );
            }
            self.cursor = self.payload.len();
            return self.missing(name, field_type, description);
        }
        self.cursor = end;
        let mut values = Vec::new();
        for byte in &self.payload[start..end] {
            values.push(format!("{:X}", byte & 0x0F));
            if values.len() < count {
                values.push(format!("{:X}", byte >> 4));
            }
        }
        ParsedField {
            name: Cow::Borrowed(name),
            field_type: Cow::Owned(field_type),
            value: array_summary(count, values.into_iter().take(16).collect()).into(),
            description: Cow::Borrowed(description),
            offset: Some(self.payload_start + start as u64),
            length: Some(byte_count as u16),
        }
    }

    fn empty_array(
        &self,
        name: impl Into<Cow<'static, str>>,
        field_type: impl Into<Cow<'static, str>>,
        description: impl Into<Cow<'static, str>>,
        start: usize,
    ) -> ParsedField {
        ParsedField {
            name: name.into(),
            field_type: field_type.into(),
            value: CompactString::const_new("count=0, preview=[]"),
            description: description.into(),
            offset: Some(self.payload_start + start as u64),
            length: Some(0),
        }
    }

    fn fixed(
        &mut self,
        name: &'static str,
        field_type: &'static str,
        description: &'static str,
        len: usize,
        read: impl Fn(&[u8]) -> CompactString,
    ) -> ParsedField {
        let start = self.cursor;
        let end = start + len;
        if end > self.payload.len() {
            if start < self.payload.len() {
                self.had_truncation = true;
                self.mark_issue(
                    RecordParseIssueKind::FixedFieldTruncated,
                    name,
                    len,
                    self.payload.len() - start,
                );
            }
            self.cursor = self.payload.len();
            return self.missing(name, field_type, description);
        }
        self.cursor = end;
        ParsedField {
            name: Cow::Borrowed(name),
            field_type: Cow::Borrowed(field_type),
            value: read(&self.payload[start..end]),
            description: Cow::Borrowed(description),
            offset: Some(self.payload_start + start as u64),
            length: Some(len as u16),
        }
    }

    fn missing(
        &self,
        name: impl Into<Cow<'static, str>>,
        field_type: impl Into<Cow<'static, str>>,
        description: impl Into<Cow<'static, str>>,
    ) -> ParsedField {
        ParsedField {
            name: name.into(),
            field_type: field_type.into(),
            value: CompactString::default(),
            description: description.into(),
            offset: None,
            length: None,
        }
    }
}

fn minimum_field_bytes(kind: FieldKind) -> usize {
    match kind {
        FieldKind::U1 | FieldKind::I1 | FieldKind::C1 | FieldKind::B1 => 1,
        FieldKind::U2 | FieldKind::I2 | FieldKind::Dn => 2,
        FieldKind::U4 | FieldKind::I4 | FieldKind::R4 => 4,
        FieldKind::Cn | FieldKind::Bn => 1,
        FieldKind::ArrayU1(_)
        | FieldKind::ArrayU2(_)
        | FieldKind::ArrayR4(_)
        | FieldKind::ArrayCn(_)
        | FieldKind::ArrayN1(_) => 0,
    }
}

fn count_value(values: &[(&'static str, usize)], name: &'static str) -> usize {
    values
        .iter()
        .rev()
        .find_map(|(field_name, value)| (*field_name == name).then_some(*value))
        .unwrap_or(0)
}

fn array_type(
    values: &[(&'static str, usize)],
    count_name: &'static str,
    item_type: &str,
) -> String {
    format!("{}x{}", count_value(values, count_name), item_type)
}

fn array_summary(count: usize, values: Vec<String>) -> String {
    let suffix = if count > values.len() { ", ..." } else { "" };
    format!("count={}, preview=[{}{}]", count, values.join(", "), suffix)
}

fn bytes_summary(bytes: &[u8], declared_len: usize) -> String {
    format!("bytes={}, preview={}", declared_len, hex_preview(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::Cursor;
    use std::path::PathBuf;

    fn record(rec_typ: u8, rec_sub: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        bytes.push(rec_typ);
        bytes.push(rec_sub);
        bytes.extend_from_slice(payload);
        bytes
    }

    fn cn(value: &str) -> Vec<u8> {
        let mut bytes = vec![value.len() as u8];
        bytes.extend_from_slice(value.as_bytes());
        bytes
    }

    fn dn(bit_count: u16, value: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&bit_count.to_le_bytes());
        bytes.extend_from_slice(value);
        bytes
    }

    fn field<'a>(record: &'a ParsedRecord, name: &str) -> &'a ParsedField {
        record
            .fields
            .iter()
            .find(|field| field.name == name)
            .unwrap_or_else(|| panic!("missing field {name} in {}", record.record_type))
    }

    #[test]
    fn parser_reads_complete_stdf_v4_field_shapes() {
        let mut mir = Vec::new();
        mir.extend_from_slice(&1_u32.to_le_bytes());
        mir.extend_from_slice(&2_u32.to_le_bytes());
        mir.push(7);
        mir.extend_from_slice(b"PNQ");
        mir.extend_from_slice(&30_u16.to_le_bytes());
        mir.push(b'C');
        for value in [
            "LOT1", "PART", "NODE", "TSTR", "JOB", "REV", "SUB", "OPER", "EXEC", "1.0", "CP",
            "25C", "TXT", "AUX", "PKG", "FAM", "DATE", "FAC", "FLR", "PROC", "FREQ", "SPEC",
            "SPECV", "FLOW", "SETUP", "DSGN", "ENG", "ROM", "SER", "SUPR",
        ] {
            mir.extend(cn(value));
        }

        let mut mpr = Vec::new();
        mpr.extend_from_slice(&900_u32.to_le_bytes());
        mpr.extend_from_slice(&[1, 2, 0b1000_0000, 0]);
        mpr.extend_from_slice(&3_u16.to_le_bytes());
        mpr.extend_from_slice(&2_u16.to_le_bytes());
        mpr.extend_from_slice(&[0x21, 0x03]);
        mpr.extend_from_slice(&1.5_f32.to_le_bytes());
        mpr.extend_from_slice(&2.5_f32.to_le_bytes());
        mpr.extend(cn("multi"));
        mpr.extend(cn("alarm"));
        mpr.push(0);
        mpr.extend_from_slice(&[-6_i8 as u8, -6_i8 as u8, -6_i8 as u8]);
        for value in [0.1_f32, 9.9, 0.0, 1.0] {
            mpr.extend_from_slice(&value.to_le_bytes());
        }
        for value in [10_u16, 11, 12] {
            mpr.extend_from_slice(&value.to_le_bytes());
        }
        for value in ["V", "VIN", "%.3f", "%.2f", "%.2f"] {
            mpr.extend(cn(value));
        }
        mpr.extend_from_slice(&0.0_f32.to_le_bytes());
        mpr.extend_from_slice(&10.0_f32.to_le_bytes());

        let mut ftr = Vec::new();
        ftr.extend_from_slice(&901_u32.to_le_bytes());
        ftr.extend_from_slice(&[1, 2, 0b1000_0000, 0b1100_0000]);
        for value in [1_u32, 2, 3, 4] {
            ftr.extend_from_slice(&value.to_le_bytes());
        }
        ftr.extend_from_slice(&(-1_i32).to_le_bytes());
        ftr.extend_from_slice(&(-2_i32).to_le_bytes());
        ftr.extend_from_slice(&(-3_i16).to_le_bytes());
        ftr.extend_from_slice(&2_u16.to_le_bytes());
        ftr.extend_from_slice(&2_u16.to_le_bytes());
        for value in [1_u16, 2] {
            ftr.extend_from_slice(&value.to_le_bytes());
        }
        ftr.push(0x21);
        for value in [3_u16, 4] {
            ftr.extend_from_slice(&value.to_le_bytes());
        }
        ftr.push(0x43);
        ftr.extend(dn(10, &[0b1010_1010, 0b0000_0011]));
        for value in ["vec", "time", "op", "test", "alarm", "prog", "rslt"] {
            ftr.extend(cn(value));
        }
        ftr.push(5);
        ftr.extend(dn(3, &[0b0000_0101]));

        let mut gdr = Vec::new();
        gdr.extend_from_slice(&4_u16.to_le_bytes());
        gdr.extend_from_slice(&[1, 255]);
        gdr.push(10);
        gdr.extend(cn("AB"));
        gdr.push(12);
        gdr.extend(dn(3, &[0b0000_0101]));
        gdr.push(13);
        gdr.push(0x0A);

        let mut bytes = Vec::new();
        bytes.extend(record(1, 10, &mir));
        bytes.extend(record(15, 15, &mpr));
        bytes.extend(record(15, 20, &ftr));
        bytes.extend(record(50, 10, &gdr));

        let mut parsed = Vec::new();
        parse_reader(
            &mut Cursor::new(bytes.clone()),
            bytes.len() as u64,
            |parsed_record| {
                parsed.push(parsed_record);
                true
            },
            |_, _| {},
        )
        .expect("fixture should parse");

        assert_eq!(field(&parsed[0], "STAT_NUM").field_type, "U*1");
        assert_eq!(field(&parsed[0], "MODE_COD").field_type, "C*1");
        assert_eq!(field(&parsed[0], "MODE_COD").value, "P");
        assert_eq!(field(&parsed[0], "LOT_ID").value, "LOT1");
        assert_eq!(field(&parsed[1], "RTN_STAT").field_type, "3xN*1");
        assert!(field(&parsed[1], "RTN_STAT").value.contains("count=3"));
        assert!(field(&parsed[1], "RTN_RSLT").value.contains("1.5"));
        assert_eq!(field(&parsed[2], "TEST_FLG").field_type, "B*1");
        assert!(field(&parsed[2], "FAIL_PIN").value.contains("bits=10"));
        assert!(parsed[3].fields.is_empty());
    }

    #[test]
    fn parser_emits_omitted_optional_tail_fields_without_error() {
        let mut ptr = Vec::new();
        ptr.extend_from_slice(&100_u32.to_le_bytes());
        ptr.extend_from_slice(&[1, 2, 0, 0]);
        ptr.extend_from_slice(&1.25_f32.to_le_bytes());
        ptr.extend(cn("VDD"));
        ptr.extend(cn(""));

        let mut parsed = Vec::new();
        parse_reader(
            &mut Cursor::new(record(15, 10, &ptr)),
            0,
            |parsed_record| {
                parsed.push(parsed_record);
                true
            },
            |_, _| {},
        )
        .expect("fixture should parse");

        assert_eq!(parsed[0].status, RecordStatus::Parsed);
        assert_eq!(field(&parsed[0], "OPT_FLAG").value, "");
        assert_eq!(field(&parsed[0], "HI_SPEC").offset, None);
    }

    #[test]
    fn parser_marks_standard_record_error_when_required_field_is_truncated() {
        let mut parsed = Vec::new();
        parse_reader(
            &mut Cursor::new(record(0, 10, &[2])),
            0,
            |parsed_record| {
                parsed.push(parsed_record);
                true
            },
            |_, _| {},
        )
        .expect("record framing should parse");

        assert_eq!(parsed[0].record_type, "FAR");
        assert_eq!(parsed[0].status, RecordStatus::Error);
        assert_eq!(field(&parsed[0], "STDF_VER").value, "");
    }

    #[test]
    fn parser_leaves_dtr_unexpanded_by_default() {
        let mut parsed = Vec::new();
        parse_reader(
            &mut Cursor::new(record(50, 30, &[5, b'A'])),
            0,
            |parsed_record| {
                parsed.push(parsed_record);
                true
            },
            |_, _| {},
        )
        .expect("record framing should parse");

        assert_eq!(parsed[0].record_type, "DTR");
        assert_eq!(parsed[0].status, RecordStatus::Parsed);
        assert!(parsed[0].fields.is_empty());
    }

    #[test]
    #[ignore = "requires STDF_SAMPLE_PATH or the local customer sample file"]
    fn parser_reads_real_customer_sample_without_standard_raw_records() {
        let path = match std::env::var("STDF_SAMPLE_PATH") {
            Ok(p) => PathBuf::from(p),
            Err(_) => {
                eprintln!("skipped: set STDF_SAMPLE_PATH to run this benchmark");
                return;
            }
        };
        let metadata = std::fs::metadata(&path).expect("sample metadata");
        let mut file = File::open(&path).expect("open sample");
        let mut counts = HashMap::<String, usize>::new();
        let mut raw_standard_records = 0_usize;
        let mut unknown_records = 0_usize;

        parse_reader(
            &mut file,
            metadata.len(),
            |record| {
                if record.record_type == "UNKNOWN" {
                    unknown_records += 1;
                }
                if record.record_type != "UNKNOWN"
                    && record.fields.iter().any(|field| field.name == "RAW_BYTES")
                {
                    raw_standard_records += 1;
                }
                *counts.entry(record.record_type.to_string()).or_insert(0) += 1;
                true
            },
            |_, _| {},
        )
        .expect("sample should parse");

        assert_eq!(unknown_records, 0);
        assert_eq!(raw_standard_records, 0);
        assert_eq!(counts.get("PTR").copied(), Some(158_009));
        assert_eq!(counts.get("FTR").copied(), Some(95_149));
        assert_eq!(counts.get("DTR").copied(), Some(106_537));
        assert_eq!(counts.get("TSR").copied(), Some(68_940));
        assert_eq!(counts.get("PRR").copied(), Some(32));
    }
}
