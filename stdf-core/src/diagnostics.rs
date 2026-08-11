use crate::parser::{
    record_name, ParsedField, ParsedRecord, ParserError, RecordParseIssueKind, RecordStatus,
};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;

const MAX_SAMPLES_PER_ISSUE: usize = 3;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FileIssueSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileIssueLocation {
    pub offset: u64,
    pub record_index: Option<usize>,
    pub record_type: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileIssue {
    pub code: String,
    pub severity: FileIssueSeverity,
    pub title: String,
    pub actual: String,
    pub expected: String,
    pub message: String,
    pub suggestion: String,
    pub count: usize,
    pub affected_records: usize,
    pub affects_accuracy: bool,
    pub samples: Vec<FileIssueLocation>,
}

#[derive(Debug, Clone, Copy)]
struct RecordMeta {
    offset: u64,
    length: u16,
    rec_typ: u8,
    rec_sub: u8,
    record_type: &'static str,
}

#[derive(Debug, Clone)]
struct SuspiciousTextSplit {
    field_name: String,
    following_field_name: String,
    declared_len: usize,
    expected_len: usize,
    field_offset: usize,
    text: String,
    following_len: usize,
    following_prefix: Vec<u8>,
}

#[derive(Debug, Clone)]
struct BoundaryFieldEvidence {
    field_name: String,
    following_field_name: String,
    declared_len: usize,
    expected_len: usize,
    text: String,
    following_text: String,
}

#[derive(Debug, Clone)]
struct BoundaryEvidence {
    previous: RecordMeta,
    previous_expected_len: usize,
    field: Option<BoundaryFieldEvidence>,
    unknown_offset: u64,
    apparent_header: [u8; 4],
    apparent_len: u16,
    apparent_typ: u8,
    apparent_sub: u8,
    boundary_shift: usize,
    next_offset: u64,
    next_len: u16,
    next_typ: u8,
    next_sub: u8,
    next_name: &'static str,
}

#[derive(Debug, Clone)]
struct SemanticCheckpoint {
    issues: Vec<FileIssue>,
    issue_index: FxHashMap<&'static str, usize>,
    far_count: usize,
    mir_count: usize,
    mrr_count: usize,
    seen_mrr: bool,
    start_time: Option<u64>,
    finish_time: Option<u64>,
    open_parts: FxHashMap<(u8, u8), OpenPart>,
    open_wafers: FxHashMap<(u8, u8), (u64, usize)>,
    open_bps: Vec<(u64, usize)>,
    pmr_indices: FxHashSet<u16>,
    pmr_definitions: FxHashSet<(u16, u8, u8)>,
    referenced_pmr: FxHashMap<u16, u64>,
    checked_limits: FxHashSet<(&'static str, u32)>,
}

#[derive(Debug, Clone, Copy)]
struct OpenPart {
    offset: u64,
    record_index: usize,
    test_count: usize,
}

#[derive(Default)]
pub struct FileIssueCollector {
    issues: Vec<FileIssue>,
    issue_index: FxHashMap<&'static str, usize>,
    previous: Option<RecordMeta>,
    previous_text_split: Option<SuspiciousTextSplit>,
    record_count: usize,
    far_count: usize,
    mir_count: usize,
    mrr_count: usize,
    seen_mrr: bool,
    start_time: Option<u64>,
    finish_time: Option<u64>,
    open_parts: FxHashMap<(u8, u8), OpenPart>,
    open_wafers: FxHashMap<(u8, u8), (u64, usize)>,
    open_bps: Vec<(u64, usize)>,
    pmr_indices: FxHashSet<u16>,
    pmr_definitions: FxHashSet<(u16, u8, u8)>,
    referenced_pmr: FxHashMap<u16, u64>,
    checked_limits: FxHashSet<(&'static str, u32)>,
    first_unknown_offset: Option<u64>,
    nonstandard_count: usize,
    nonstandard_pairs: FxHashSet<(u8, u8)>,
    boundary_collapsed: bool,
    boundary_evidence: Option<BoundaryEvidence>,
    semantic_checkpoint: Option<Box<SemanticCheckpoint>>,
}

impl FileIssueCollector {
    pub fn observe(&mut self, record: &ParsedRecord, record_index: usize) {
        self.record_count += 1;

        if record_index == 0 && record.record_type != "FAR" {
            self.push(
                "missing_initial_far",
                FileIssueSeverity::Error,
                "文件没有从标准起点开始",
                format!("第一条记录是 {}，不是 FAR。", record.record_type),
                "一份完整的 STDF V4 文件应以 FAR 作为第一条记录。",
                "文件可能缺少开头、被截取，或并非完整的 STDF，因此版本和字节序无法从标准起点确认。",
                "建议重新导出完整文件，并确认传输过程中没有被截断。",
                true,
                location(record, record_index, "这是软件读到的第一条记录。"),
            );
        }

        if !self.boundary_untrusted() {
            if let Some(issue) = &record.parse_issue {
                let shortage = issue.expected_bytes.saturating_sub(issue.remaining_bytes);
                let minimum_rec_len = usize::from(record.length).saturating_add(shortage);
                let standard_rec_len =
                    usize::from(record.length).saturating_sub(issue.remaining_bytes);
                let (title, actual, expected, message) = match issue.kind {
                    RecordParseIssueKind::RequiredFieldMissing => (
                        "记录缺少必填内容",
                        format!(
                            "{} 的 REC_LEN={}；读取到必填字段 {} 时只剩 {} 字节。",
                            record.record_type, record.length, issue.field_name, issue.remaining_bytes
                        ),
                        format!(
                            "字段 {} 至少需要 {} 字节，因此 REC_LEN 至少应为 {}。",
                            issue.field_name, issue.expected_bytes, minimum_rec_len
                        ),
                        format!(
                            "当前记录比完成必填字段至少少 {shortage} 字节，记录内容不完整。软件会继续读取后面的记录。"
                        ),
                    ),
                    RecordParseIssueKind::FixedFieldTruncated => (
                        "字段在记录中途结束",
                        format!(
                            "{} 的 REC_LEN={}；字段 {} 实际只有 {} 字节可读。",
                            record.record_type, record.length, issue.field_name, issue.remaining_bytes
                        ),
                        format!(
                            "字段 {} 需要 {} 字节，因此 REC_LEN 至少应为 {}。",
                            issue.field_name, issue.expected_bytes, minimum_rec_len
                        ),
                        format!("该字段少了 {shortage} 字节，无法按 STDF V4 字段结构完整解释。"),
                    ),
                    RecordParseIssueKind::DeclaredLengthExceedsRecord => (
                        "字段声明的长度超出记录范围",
                        format!(
                            "{} 的 REC_LEN={}；字段 {} 自己声明有 {} 字节，但实际只剩 {} 字节。",
                            record.record_type,
                            record.length,
                            issue.field_name,
                            issue.expected_bytes,
                            issue.remaining_bytes
                        ),
                        format!(
                            "要满足字段声明，REC_LEN 至少应为 {}，或字段声明长度应不超过 {} 字节。",
                            minimum_rec_len, issue.remaining_bytes
                        ),
                        format!("字段声明与记录边界相差 {shortage} 字节，至少有一处长度写错。"),
                    ),
                    RecordParseIssueKind::ArrayExceedsRecord => (
                        "数组内容没有完整写入",
                        format!(
                            "{} 的 REC_LEN={}；数组 {} 声明需要 {} 字节，但实际只剩 {} 字节。",
                            record.record_type,
                            record.length,
                            issue.field_name,
                            issue.expected_bytes,
                            issue.remaining_bytes
                        ),
                        format!(
                            "要容纳完整数组，REC_LEN 至少应为 {}，还需要增加 {shortage} 字节。",
                            minimum_rec_len
                        ),
                        "数组计数或 REC_LEN 与实际写入内容不一致，数组后面的字段也可能无法正确读取。".to_string(),
                    ),
                    RecordParseIssueKind::UnexpectedTrailingBytes => (
                        "记录末尾存在无法解释的内容",
                        format!(
                            "{} 的 REC_LEN={}；标准字段结束后仍多出 {} 字节。",
                            record.record_type, record.length, issue.remaining_bytes
                        ),
                        format!(
                            "若只包含 STDF V4 标准字段，REC_LEN 应为 {standard_rec_len}；若多出的内容是厂商扩展，则应有对应的扩展定义。"
                        ),
                        "多出的字节无法对应到当前记录的标准字段，可能是厂商扩展，也可能是 REC_LEN 写大。".to_string(),
                    ),
                };
                let severity =
                    if matches!(issue.kind, RecordParseIssueKind::UnexpectedTrailingBytes) {
                        FileIssueSeverity::Warning
                    } else {
                        FileIssueSeverity::Error
                    };
                self.push(
                    if matches!(issue.kind, RecordParseIssueKind::UnexpectedTrailingBytes) {
                        "unexpected_record_tail"
                    } else {
                        "record_field_incomplete"
                    },
                    severity,
                    title,
                    actual,
                    expected,
                    message,
                    "建议让文件生成方检查该 record 的长度和字段写入逻辑。",
                    !matches!(issue.kind, RecordParseIssueKind::UnexpectedTrailingBytes),
                    location(
                        record,
                        record_index,
                        format!(
                            "字段 {}，需要 {} 字节，实际可用 {} 字节。",
                            issue.field_name, issue.expected_bytes, issue.remaining_bytes
                        ),
                    ),
                );
            }
        }

        if record.status == RecordStatus::Unknown {
            if self.first_unknown_offset.is_none() {
                self.semantic_checkpoint = Some(Box::new(self.capture_semantic_checkpoint()));
            }
            self.nonstandard_count += 1;
            self.nonstandard_pairs
                .insert((record.rec_typ, record.rec_sub));
            self.first_unknown_offset.get_or_insert(record.offset);
            if self.boundary_evidence.is_none() && !self.boundary_collapsed {
                self.boundary_evidence = self.infer_boundary_evidence(record);
            }
            if self.boundary_collapsed {
                self.refresh_boundary_counts();
            } else {
                let previous = self.previous.map(|item| {
                    format!(
                        "上一条为 {}（REC_TYP={}, REC_SUB={}, REC_LEN={}），位于 byte {}。",
                        item.record_type, item.rec_typ, item.rec_sub, item.length, item.offset
                    )
                });
                self.push(
                    "nonstandard_record",
                    FileIssueSeverity::Warning,
                    "发现非标准或无法识别的记录",
                    format!(
                        "记录头写的是 REC_TYP={}、REC_SUB={}、REC_LEN={}，该类型不在当前支持的 STDF V4 标准表中。",
                        record.rec_typ, record.rec_sub, record.length
                    ),
                    "REC_TYP/REC_SUB 应对应标准 STDF V4 记录，或对应已明确支持的厂商扩展；仅凭损坏后的内容无法可靠反推出正确 REC_LEN。",
                    "它可能是厂商扩展；如果数量很多，通常表示更早的 REC_LEN 或字段长度写错，后续内容被误当成记录头。",
                    "可以先查看第一处位置；若同类问题连续大量出现，建议让文件生成方重新导出。",
                    true,
                    location(
                        record,
                        record_index,
                        format!(
                            "REC_TYP={}, REC_SUB={}, REC_LEN={}。{}",
                            record.rec_typ,
                            record.rec_sub,
                            record.length,
                            previous.unwrap_or_default()
                        ),
                    ),
                );
                if self.nonstandard_count == 1 && self.boundary_evidence.is_some() {
                    self.promote_boundary_issue(false);
                }
            }
        }

        if self.boundary_untrusted() {
            if !self.boundary_collapsed {
                self.restore_semantic_checkpoint();
                self.promote_boundary_issue(false);
                self.boundary_collapsed = true;
            }
            return;
        }

        self.check_non_finite_values(record, record_index);

        match record.record_type {
            "FAR" => self.observe_far(record, record_index),
            "MIR" => {
                self.mir_count += 1;
                self.start_time = parse_u64(field_value(&record.fields, "START_T"));
                if self.mir_count > 1 {
                    self.push(
                        "duplicate_mir",
                        FileIssueSeverity::Warning,
                        "文件中出现了多条 MIR",
                        format!("文件中已读到 {} 条 MIR。", self.mir_count),
                        "一份独立的 STDF 测试数据通常只有 1 条 MIR。",
                        "多条主信息记录可能表示多个测试文件被直接拼接，批次信息的归属会变得不明确。",
                        "建议确认文件是否由多个测试批次合并而成。",
                        false,
                        location(record, record_index, "这是重复出现的 MIR。"),
                    );
                }
            }
            "MRR" => {
                self.mrr_count += 1;
                self.seen_mrr = true;
                self.finish_time = parse_u64(field_value(&record.fields, "FINISH_T"));
                if self.mrr_count > 1 {
                    self.push(
                        "duplicate_mrr",
                        FileIssueSeverity::Warning,
                        "文件中出现了多条结束记录",
                        format!("文件中已读到 {} 条 MRR。", self.mrr_count),
                        "一份独立的 STDF 测试数据通常只有 1 条 MRR，并位于数据结尾。",
                        "多条结束记录可能意味着多个 STDF 内容被拼接，软件无法把它们视为单一连续批次。",
                        "建议确认文件是否包含多个测试批次。",
                        false,
                        location(record, record_index, "这是重复出现的 MRR。"),
                    );
                }
            }
            "PIR" => self.open_part(record, record_index),
            "PTR" | "MPR" | "FTR" => {
                self.observe_test_record(record, record_index);
                self.check_limits(record, record_index);
                self.collect_pmr_references(record);
            }
            "PRR" => self.close_part(record, record_index),
            "WIR" => self.open_wafer(record, record_index),
            "WRR" => self.close_wafer(record, record_index),
            "BPS" => self.open_bps.push((record.offset, record_index)),
            "EPS" => {
                if self.open_bps.pop().is_none() {
                    self.push(
                        "eps_without_bps",
                        FileIssueSeverity::Warning,
                        "测试段结束记录没有对应的开始记录",
                        "当前 EPS 出现时，没有任何尚未结束的 BPS 可以与它配对。",
                        "每条 EPS 前面都应有一条对应且尚未结束的 BPS。",
                        "测试段的开始记录可能缺失，或结束记录出现顺序不正确。",
                        "建议检查测试程序导出的 BPS/EPS 是否成对。",
                        false,
                        location(record, record_index, "EPS 前没有可配对的 BPS。"),
                    );
                }
            }
            "PMR" => self.observe_pmr(record, record_index),
            _ => {}
        }

        if self.seen_mrr && record.record_type != "MRR" {
            self.push(
                "records_after_mrr",
                FileIssueSeverity::Warning,
                "文件结束记录后仍有其他内容",
                format!("MRR 之后又出现了 {} 记录。", record.record_type),
                "MRR 应是这份 STDF 测试数据的最后一条记录。",
                "文件可能被拼接，或 MRR 写入位置不正确，因此结束标记之后的数据归属不明确。",
                "建议确认 MRR 后面的内容是否属于另一份测试数据。",
                false,
                location(record, record_index, "这条记录出现在 MRR 之后。"),
            );
        }

        self.remember(record);
    }

    fn remember(&mut self, record: &ParsedRecord) {
        self.previous_text_split = suspicious_text_split(record);
        self.previous = Some(RecordMeta {
            offset: record.offset,
            length: record.length,
            rec_typ: record.rec_typ,
            rec_sub: record.rec_sub,
            record_type: record.record_type,
        });
    }

    pub fn finish(&mut self, parser_error: Option<&ParserError>) {
        let boundary_untrusted = self.boundary_untrusted();
        if self.record_count == 0 {
            self.push_without_location(
                "empty_file",
                FileIssueSeverity::Error,
                "文件中没有可读取的记录",
                "软件读到的完整 STDF 记录数为 0。",
                "文件至少应包含完整的 FAR，并通常还应包含 MIR、测试记录和 MRR。",
                "文件可能为空、格式不正确，或只包含不足以组成一条记录的残缺数据。",
                "请确认选择的是原始 STDF 文件，并尝试重新导出或重新传输。",
                true,
            );
        }
        if !boundary_untrusted && self.far_count == 0 && self.record_count > 0 {
            self.push_without_location(
                "missing_far",
                FileIssueSeverity::Error,
                "文件缺少 FAR 基本信息",
                format!("已读取 {} 条记录，但 FAR 数量为 0。", self.record_count),
                "完整 STDF 文件应以 1 条 FAR 开头，用来声明 CPU 类型和 STDF 版本。",
                "没有 FAR 就无法按标准确认版本和数据字节序，后续数值解释可能不可靠。",
                "建议让文件生成方重新导出完整 STDF。",
                true,
            );
        }
        if !boundary_untrusted && self.mir_count == 0 && self.record_count > 0 {
            self.push_without_location(
                "missing_mir",
                FileIssueSeverity::Warning,
                "文件缺少测试主信息",
                "整份文件中 MIR 数量为 0。",
                "一份完整的测试数据通常应包含 1 条 MIR。",
                "缺少主信息记录，批次、产品、测试程序和开始时间等关键信息可能无法显示。",
                "建议确认文件是否从测试中途截取。",
                false,
            );
        }
        if !boundary_untrusted && self.mrr_count == 0 && self.record_count > 0 {
            self.push_without_location(
                "missing_mrr",
                FileIssueSeverity::Warning,
                "文件缺少正常结束标记",
                "整份文件中 MRR 数量为 0。",
                "一份正常结束的测试数据通常应包含 1 条 MRR，并位于数据结尾。",
                "文件可能在测试结束前被截断，或生成程序没有写入结束记录。",
                "建议确认测试是否完整结束，并重新导出文件。",
                false,
            );
        }

        if !boundary_untrusted && !self.open_parts.is_empty() {
            let count = self.open_parts.len();
            let first = self
                .open_parts
                .values()
                .min_by_key(|item| item.offset)
                .copied();
            self.push(
                "unclosed_parts",
                FileIssueSeverity::Warning,
                "有器件记录没有正常结束",
                format!("解析结束时仍有 {count} 个 PIR 没有对应的 PRR。"),
                "每个 PIR 都应由同一 head/site 的 PRR 正常结束。",
                "这些器件的 bin、测试耗时和器件编号可能不完整。",
                "建议检查文件是否中途结束，以及 PIR/PRR 是否按器件成对写入。",
                false,
                first.map(|item| FileIssueLocation {
                    offset: item.offset,
                    record_index: Some(item.record_index),
                    record_type: "PIR".to_string(),
                    detail: "这是最早尚未配对的 PIR。".to_string(),
                }),
            );
        }
        if !boundary_untrusted && !self.open_wafers.is_empty() {
            let first = self.open_wafers.values().min_by_key(|item| item.0).copied();
            self.push(
                "unclosed_wafers",
                FileIssueSeverity::Warning,
                "有晶圆记录没有正常结束",
                format!(
                    "解析结束时仍有 {} 条 WIR 没有对应的 WRR。",
                    self.open_wafers.len()
                ),
                "每条 WIR 都应由同一 head/site group 的 WRR 正常结束。",
                "晶圆汇总数量和结束时间可能不完整。",
                "建议检查文件是否中途结束，以及 WIR/WRR 是否按晶圆成对写入。",
                false,
                first.map(|item| FileIssueLocation {
                    offset: item.0,
                    record_index: Some(item.1),
                    record_type: "WIR".to_string(),
                    detail: "这是最早尚未配对的 WIR。".to_string(),
                }),
            );
        }
        if !boundary_untrusted && !self.open_bps.is_empty() {
            let first = self.open_bps.first().copied();
            self.push(
                "unclosed_bps",
                FileIssueSeverity::Warning,
                "有测试段没有正常结束",
                format!(
                    "解析结束时仍有 {} 条 BPS 没有对应的 EPS。",
                    self.open_bps.len()
                ),
                "每条 BPS 都应由后续 EPS 正常结束。",
                "测试段名称和范围可能不完整。",
                "建议检查文件是否中途结束，以及 BPS/EPS 是否按测试段成对写入。",
                false,
                first.map(|item| FileIssueLocation {
                    offset: item.0,
                    record_index: Some(item.1),
                    record_type: "BPS".to_string(),
                    detail: "这是最早尚未配对的 BPS。".to_string(),
                }),
            );
        }

        if !boundary_untrusted {
            let missing_pmr: Vec<(u16, u64)> = self
                .referenced_pmr
                .iter()
                .filter(|(index, _)| !self.pmr_indices.contains(index))
                .map(|(index, offset)| (*index, *offset))
                .take(MAX_SAMPLES_PER_ISSUE)
                .collect();
            for (index, offset) in missing_pmr {
                self.push(
                    "missing_pmr_reference",
                    FileIssueSeverity::Warning,
                    "测试记录引用了不存在的 pin",
                    format!(
                        "MPR 或 FTR 引用了 PMR 索引 {index}，但整份文件中没有该索引的 PMR 定义。"
                    ),
                    format!("被引用的 PMR 索引 {index} 应在 PMR 记录中预先定义。"),
                    "软件无法把该索引解析为具体 pin 名称，相关 pin 结果只能保留编号。",
                    "建议检查 PMR 是否缺失，或 pin 索引是否由生成程序写错。",
                    false,
                    Some(FileIssueLocation {
                        offset,
                        record_index: None,
                        record_type: "MPR/FTR".to_string(),
                        detail: format!("找不到 PMR 索引 {index}。"),
                    }),
                );
            }

            if let (Some(start), Some(finish)) = (self.start_time, self.finish_time) {
                if finish < start {
                    self.push_without_location(
                        "time_order_invalid",
                        FileIssueSeverity::Warning,
                        "测试结束时间早于开始时间",
                        format!("MIR 的开始时间为 {start}，MRR 的结束时间为 {finish}。"),
                        format!("结束时间应大于或等于开始时间，即至少为 {start}。"),
                        "当前结束时间早于开始时间，文件中的时间线不成立。",
                        "建议检查测试机时间设置和文件生成时间。",
                        false,
                    );
                }
            }
        }

        if let Some(error) = parser_error {
            if !boundary_untrusted || matches!(error, ParserError::Io(_)) {
                self.observe_parser_error(error);
            }
        }

        if boundary_untrusted {
            self.promote_boundary_issue(true);
        }

        // Once many non-standard headers appear after one point, later
        // record-level checks are no longer independent evidence: random
        // payload bytes can resemble PTR/PIR/etc. Keep the first boundary
        // problem and any issues proven before it, but collapse downstream
        // consequences into the single nonstandard_record summary.
        if boundary_untrusted {
            self.boundary_collapsed = true;
        }
        self.refresh_duplicate_summaries();
        self.semantic_checkpoint = None;
        self.issues.sort_by_key(|issue| {
            let severity = match issue.severity {
                FileIssueSeverity::Error => 0_u8,
                FileIssueSeverity::Warning => 1_u8,
            };
            let accuracy = if issue.affects_accuracy { 0_u8 } else { 1_u8 };
            let offset = issue
                .samples
                .first()
                .map(|sample| sample.offset)
                .unwrap_or(u64::MAX);
            (severity, accuracy, offset)
        });
    }

    pub fn issues(&self) -> &[FileIssue] {
        &self.issues
    }

    fn observe_far(&mut self, record: &ParsedRecord, record_index: usize) {
        self.far_count += 1;
        if self.far_count > 1 {
            self.push(
                "duplicate_far",
                FileIssueSeverity::Warning,
                "文件中出现了多个 STDF 起点",
                format!("文件中已读到 {} 条 FAR。", self.far_count),
                "一份独立的 STDF 文件应只有 1 条 FAR，并且位于第一条记录。",
                "多个起点通常表示多份 STDF 被直接拼接，后续批次边界可能不清楚。",
                "建议确认是否需要拆分为多份 STDF 文件。",
                false,
                location(record, record_index, "这是重复出现的 FAR。"),
            );
        }
        let cpu = field_value(&record.fields, "CPU_TYPE");
        if !cpu.is_empty() && cpu != "2" {
            self.push(
                "unsupported_cpu_type",
                FileIssueSeverity::Error,
                "文件使用了当前不支持的数据格式",
                format!("FAR 中的 CPU_TYPE={cpu}。"),
                "当前版本要求 CPU_TYPE=2，即 IBM PC/little-endian 字节序。",
                "当前字节序不受可靠支持，继续解释多字节数值可能得到错误结果。",
                "建议让生成方导出 IBM PC/little-endian 格式的 STDF V4 文件。",
                true,
                location(record, record_index, format!("CPU_TYPE={cpu}。")),
            );
        }
        let version = field_value(&record.fields, "STDF_VER");
        if !version.is_empty() && version != "4" {
            self.push(
                "unsupported_stdf_version",
                FileIssueSeverity::Error,
                "STDF 版本不受支持",
                format!("FAR 中声明的 STDF_VER={version}。"),
                "当前版本要求 STDF_VER=4，即 STDF V4/V4-2007。",
                "使用其他版本的字段定义解析会造成字段位置和含义错误。",
                "请改用 STDF V4 文件，或让生成方重新导出。",
                true,
                location(record, record_index, format!("STDF_VER={version}。")),
            );
        }
    }

    fn open_part(&mut self, record: &ParsedRecord, record_index: usize) {
        let key = head_site(&record.fields);
        if let Some(previous) = self.open_parts.insert(
            key,
            OpenPart {
                offset: record.offset,
                record_index,
                test_count: 0,
            },
        ) {
            self.push(
                "overlapping_pir",
                FileIssueSeverity::Warning,
                "同一测试站点重复开始了新器件",
                format!(
                    "HEAD_NUM={}、SITE_NUM={} 的上一条 PIR 尚未结束，又出现了新的 PIR。",
                    key.0, key.1
                ),
                "同一 head/site 在开始下一颗器件前，应先用 PRR 结束当前器件。",
                "上一颗器件的结束信息可能缺失，后续测试结果的器件归属可能错位。",
                "建议检查 PIR/PRR 是否按器件成对写入。",
                false,
                location(
                    record,
                    record_index,
                    format!("上一条尚未结束的 PIR 位于 byte {}。", previous.offset),
                ),
            );
        }
    }

    fn observe_test_record(&mut self, record: &ParsedRecord, record_index: usize) {
        let key = head_site(&record.fields);
        if let Some(part) = self.open_parts.get_mut(&key) {
            part.test_count += 1;
        } else {
            self.push(
                "test_without_pir",
                FileIssueSeverity::Warning,
                "测试结果找不到对应的器件开始记录",
                format!(
                    "读到 {}，但 HEAD_NUM={}、SITE_NUM={} 当前没有打开的 PIR。",
                    record.record_type, key.0, key.1
                ),
                "每条 PTR/MPR/FTR 都应位于同一 head/site 的 PIR 与 PRR 之间。",
                "该测试结果无法可靠归属到具体器件。",
                "建议检查文件是否缺少 PIR，或是否从测试中途截取。",
                false,
                location(record, record_index, "此前没有对应的 PIR。"),
            );
        }
    }

    fn close_part(&mut self, record: &ParsedRecord, record_index: usize) {
        let key = head_site(&record.fields);
        let Some(part) = self.open_parts.remove(&key) else {
            self.push(
                "prr_without_pir",
                FileIssueSeverity::Warning,
                "器件结束记录找不到对应的开始记录",
                format!(
                    "读到 PRR，但 HEAD_NUM={}、SITE_NUM={} 当前没有打开的 PIR。",
                    key.0, key.1
                ),
                "每条 PRR 都应结束同一 head/site 之前的一条 PIR。",
                "这颗器件的开始记录缺失，测试过程可能不完整。",
                "建议检查文件是否缺少 PIR，或是否从测试中途截取。",
                false,
                location(record, record_index, "此前没有对应的 PIR。"),
            );
            return;
        };
        if let Some(declared) = parse_usize(field_value(&record.fields, "NUM_TEST")) {
            if declared != part.test_count {
                self.push(
                    "part_test_count_mismatch",
                    FileIssueSeverity::Warning,
                    "器件声明的测试数量与实际记录数不同",
                    format!(
                        "PRR 的 NUM_TEST={declared}，但对应 PIR/PRR 之间实际读到 {} 条 PTR/MPR/FTR。",
                        part.test_count
                    ),
                    format!("NUM_TEST 应与实际测试记录数一致，即应为 {}。", part.test_count),
                    "声明数量与记录数量不一致，器件测试项统计可能使用了不同口径或写入错误。",
                    "若差异普遍存在，建议检查测试机计数口径和文件生成逻辑。",
                    false,
                    location(record, record_index, format!("对应 PIR 位于 byte {}。", part.offset)),
                );
            }
        }
    }

    fn open_wafer(&mut self, record: &ParsedRecord, record_index: usize) {
        let key = head_site_group(&record.fields);
        if let Some(previous) = self.open_wafers.insert(key, (record.offset, record_index)) {
            self.push(
                "overlapping_wir",
                FileIssueSeverity::Warning,
                "同一站点组重复开始了晶圆记录",
                format!(
                    "HEAD_NUM={}、SITE_GRP={} 的上一条 WIR 尚未结束，又出现了新的 WIR。",
                    key.0, key.1
                ),
                "同一 head/site group 在开始下一片晶圆前，应先用 WRR 结束当前晶圆。",
                "上一片晶圆的结束信息缺失，晶圆边界和汇总可能不完整。",
                "建议检查 WIR/WRR 是否成对写入。",
                false,
                location(
                    record,
                    record_index,
                    format!("上一条 WIR 位于 byte {}。", previous.0),
                ),
            );
        }
    }

    fn close_wafer(&mut self, record: &ParsedRecord, record_index: usize) {
        let key = head_site_group(&record.fields);
        if self.open_wafers.remove(&key).is_none() {
            self.push(
                "wrr_without_wir",
                FileIssueSeverity::Warning,
                "晶圆结束记录找不到对应的开始记录",
                format!(
                    "读到 WRR，但 HEAD_NUM={}、SITE_GRP={} 当前没有打开的 WIR。",
                    key.0, key.1
                ),
                "每条 WRR 都应结束同一 head/site group 之前的一条 WIR。",
                "这片晶圆的开始记录缺失，晶圆测试过程可能不完整。",
                "建议检查文件是否缺少 WIR，或是否从晶圆测试中途截取。",
                false,
                location(record, record_index, "此前没有对应的 WIR。"),
            );
        }
    }

    fn observe_pmr(&mut self, record: &ParsedRecord, record_index: usize) {
        let Some(index) = parse_u16(field_value(&record.fields, "PMR_INDX")) else {
            return;
        };
        self.pmr_indices.insert(index);
        let (head, site) = head_site(&record.fields);
        if !self.pmr_definitions.insert((index, head, site)) {
            self.push(
                "duplicate_pmr_index",
                FileIssueSeverity::Warning,
                "同一站点的 pin 定义重复",
                format!("PMR_INDX={index} 在 HEAD_NUM={head}、SITE_NUM={site} 中出现了不止一次。"),
                "同一 head/site 内，每个 PMR_INDX 应只定义一次。",
                "后续记录引用该索引时，无法确定应对应哪一个 pin 定义。",
                "建议检查同一测试站点内的 PMR_INDX 是否唯一。",
                false,
                location(
                    record,
                    record_index,
                    format!("重复的 PMR_INDX={index}，HEAD_NUM={head}，SITE_NUM={site}。"),
                ),
            );
        }
    }

    fn collect_pmr_references(&mut self, record: &ParsedRecord) {
        for name in ["RTN_INDX", "PGM_INDX"] {
            for value in array_preview_values(&record.fields, name) {
                if let Ok(index) = value.parse::<u16>() {
                    self.referenced_pmr.entry(index).or_insert(record.offset);
                }
            }
        }
    }

    fn check_non_finite_values(&mut self, record: &ParsedRecord, record_index: usize) {
        let opt_flag = parse_b1(field_value(&record.fields, "OPT_FLAG"));
        for field in &record.fields {
            if !field.field_type.as_ref().ends_with("R*4") && field.field_type.as_ref() != "R*4" {
                continue;
            }
            let value = field.value.trim();
            if (field.name == "LO_LIMIT" && opt_flag.is_some_and(|flag| flag & 0b0101_0000 != 0))
                || (field.name == "HI_LIMIT"
                    && opt_flag.is_some_and(|flag| flag & 0b1010_0000 != 0))
            {
                continue;
            }
            if value.eq_ignore_ascii_case("nan")
                || value.eq_ignore_ascii_case("inf")
                || value.eq_ignore_ascii_case("-inf")
            {
                self.push(
                    "non_finite_number",
                    FileIssueSeverity::Warning,
                    "记录中出现了无法用于统计的数值",
                    format!(
                        "{} 的 {} 字段值为 {value}。",
                        record.record_type, field.name
                    ),
                    format!("{} 应是一个有限的实数。", field.name),
                    "NaN 或 Infinity 无法参与正常的良率判断和范围计算，导出结果也可能受到影响。",
                    "建议检查测试机是否在无结果时写入了 NaN/Infinity。",
                    false,
                    location(
                        record,
                        record_index,
                        format!("字段 {}={value}。", field.name),
                    ),
                );
                break;
            }
        }
    }

    fn check_limits(&mut self, record: &ParsedRecord, record_index: usize) {
        let Some(test_num) = field_value(&record.fields, "TEST_NUM").parse::<u32>().ok() else {
            return;
        };
        if !self.checked_limits.insert((record.record_type, test_num)) {
            return;
        }
        let opt_flag = parse_b1(field_value(&record.fields, "OPT_FLAG"));
        let low_valid = opt_flag.is_none_or(|flag| flag & 0b0101_0000 == 0);
        let high_valid = opt_flag.is_none_or(|flag| flag & 0b1010_0000 == 0);
        let result_scale = if opt_flag.is_none_or(|flag| flag & 0b0000_0001 == 0) {
            field_value(&record.fields, "RES_SCAL")
                .parse::<i32>()
                .unwrap_or(0)
        } else {
            0
        };
        let low = low_valid
            .then(|| scaled_value(&record.fields, "LO_LIMIT", "LLM_SCAL", result_scale))
            .flatten();
        let high = high_valid
            .then(|| scaled_value(&record.fields, "HI_LIMIT", "HLM_SCAL", result_scale))
            .flatten();
        if matches!((low, high), (Some(low), Some(high)) if low > high) {
            self.push(
                "limit_order_invalid",
                FileIssueSeverity::Warning,
                "测试下限高于上限",
                format!(
                    "{} 测试号 {test_num} 的缩放后低限为 {low:?}，高限为 {high:?}。",
                    record.record_type,
                ),
                "测试下限应小于或等于测试上限。",
                "当前有效范围不存在，自动判定结果可能不符合测试程序的原意。",
                "建议检查测试程序中的 limit 和缩放指数设置。",
                false,
                location(
                    record,
                    record_index,
                    format!("缩放后低限={low:?}，高限={high:?}。"),
                ),
            );
        }
    }

    fn observe_parser_error(&mut self, error: &ParserError) {
        match error {
            ParserError::TruncatedHeader { offset, available } => self.push(
                "truncated_header",
                FileIssueSeverity::Error,
                "文件末尾只剩下半个记录头",
                format!("文件在 byte {offset} 处只剩 {available} 字节，无法组成完整记录头。"),
                "每个 STDF record header 固定需要 4 字节：2 字节 REC_LEN、1 字节 REC_TYP、1 字节 REC_SUB。",
                format!("记录头少了 {} 字节，最后一条记录无法开始解析。", 4_usize.saturating_sub(*available)),
                "文件很可能没有完整写入或传输完成，建议重新导出或重新发送。",
                true,
                Some(FileIssueLocation {
                    offset: *offset,
                    record_index: None,
                    record_type: "文件末尾".to_string(),
                    detail: format!("header 需要 4 字节，实际只有 {available} 字节。"),
                }),
            ),
            ParserError::TruncatedPayload {
                offset,
                expected,
                available,
                rec_typ,
                rec_sub,
            } => self.push(
                "truncated_payload",
                FileIssueSeverity::Error,
                "文件末尾的记录没有写完整",
                format!(
                    "记录头声明 REC_LEN={expected}，但文件中实际只剩 {available} 字节内容。"
                ),
                format!("REC_LEN={expected} 时，记录头后面应完整存在 {expected} 字节内容。"),
                format!(
                    "记录内容少了 {} 字节，最后一条记录无法完整解析；此前完整记录仍可查看。",
                    expected.saturating_sub(*available)
                ),
                "建议重新导出文件；如果测试过程中异常中止，也请检查测试机日志。",
                true,
                Some(FileIssueLocation {
                    offset: *offset,
                    record_index: None,
                    record_type: format!("REC_TYP={rec_typ}, REC_SUB={rec_sub}"),
                    detail: format!("声明 {expected} 字节，实际 {available} 字节。"),
                }),
            ),
            ParserError::UnsupportedByteOrder { offset } => self.push(
                "unsupported_byte_order",
                FileIssueSeverity::Error,
                "文件字节序暂不受支持",
                "FAR 的记录头使用了 big-endian 字节序。",
                "当前版本要求常见的 little-endian STDF V4 格式。",
                "按 little-endian 继续读取会把 REC_LEN 和多字节数值解释错误。",
                "建议让生成方导出常见的 little-endian STDF V4 文件。",
                true,
                Some(FileIssueLocation {
                    offset: *offset,
                    record_index: Some(0),
                    record_type: "FAR".to_string(),
                    detail: "FAR 的 REC_LEN 使用 big-endian 字节序。".to_string(),
                }),
            ),
            ParserError::Io(error) => self.push_without_location(
                "file_read_error",
                FileIssueSeverity::Error,
                "读取文件时发生错误",
                format!("系统返回读取错误：{error}"),
                "解析期间文件应保持可访问，并能连续读到声明的文件末尾。",
                "系统无法继续提供文件内容，解析被迫停止。",
                "请检查文件是否仍在磁盘上、是否有读取权限，以及存储设备是否正常。",
                true,
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        code: &'static str,
        severity: FileIssueSeverity,
        title: impl Into<String>,
        actual: impl Into<String>,
        expected: impl Into<String>,
        message: impl Into<String>,
        suggestion: impl Into<String>,
        affects_accuracy: bool,
        sample: Option<FileIssueLocation>,
    ) {
        let actual = actual.into();
        let expected = expected.into();
        if let Some(index) = self.issue_index.get(code).copied() {
            let issue = &mut self.issues[index];
            issue.count += 1;
            issue.affects_accuracy |= affects_accuracy;
            if issue.count == 2 && (issue.actual != actual || issue.expected != expected) {
                issue.actual = format!("不同位置的实际值不完全相同。第一处是：{}", issue.actual);
                issue.expected = format!("每一处都应满足对应规则。第一处应为：{}", issue.expected);
                if code == "record_field_incomplete" {
                    issue.title = "多条记录的字段结构不完整".to_string();
                }
            }
            if issue.samples.len() < MAX_SAMPLES_PER_ISSUE {
                if let Some(sample) = sample {
                    issue.samples.push(sample);
                }
            }
            return;
        }
        let mut samples = Vec::new();
        if let Some(sample) = sample {
            samples.push(sample);
        }
        self.issue_index.insert(code, self.issues.len());
        self.issues.push(FileIssue {
            code: code.to_string(),
            severity,
            title: title.into(),
            actual,
            expected,
            message: message.into(),
            suggestion: suggestion.into(),
            count: 1,
            affected_records: 0,
            affects_accuracy,
            samples,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn push_without_location(
        &mut self,
        code: &'static str,
        severity: FileIssueSeverity,
        title: impl Into<String>,
        actual: impl Into<String>,
        expected: impl Into<String>,
        message: impl Into<String>,
        suggestion: impl Into<String>,
        affects_accuracy: bool,
    ) {
        self.push(
            code,
            severity,
            title,
            actual,
            expected,
            message,
            suggestion,
            affects_accuracy,
            None,
        );
    }

    fn capture_semantic_checkpoint(&self) -> SemanticCheckpoint {
        SemanticCheckpoint {
            issues: self.issues.clone(),
            issue_index: self.issue_index.clone(),
            far_count: self.far_count,
            mir_count: self.mir_count,
            mrr_count: self.mrr_count,
            seen_mrr: self.seen_mrr,
            start_time: self.start_time,
            finish_time: self.finish_time,
            open_parts: self.open_parts.clone(),
            open_wafers: self.open_wafers.clone(),
            open_bps: self.open_bps.clone(),
            pmr_indices: self.pmr_indices.clone(),
            pmr_definitions: self.pmr_definitions.clone(),
            referenced_pmr: self.referenced_pmr.clone(),
            checked_limits: self.checked_limits.clone(),
        }
    }

    fn restore_semantic_checkpoint(&mut self) {
        let Some(checkpoint) = self.semantic_checkpoint.take() else {
            return;
        };
        let boundary_issue = self
            .issue_index
            .get("nonstandard_record")
            .and_then(|index| self.issues.get(*index))
            .cloned();
        self.issues = checkpoint.issues;
        self.issue_index = checkpoint.issue_index;
        self.far_count = checkpoint.far_count;
        self.mir_count = checkpoint.mir_count;
        self.mrr_count = checkpoint.mrr_count;
        self.seen_mrr = checkpoint.seen_mrr;
        self.start_time = checkpoint.start_time;
        self.finish_time = checkpoint.finish_time;
        self.open_parts = checkpoint.open_parts;
        self.open_wafers = checkpoint.open_wafers;
        self.open_bps = checkpoint.open_bps;
        self.pmr_indices = checkpoint.pmr_indices;
        self.pmr_definitions = checkpoint.pmr_definitions;
        self.referenced_pmr = checkpoint.referenced_pmr;
        self.checked_limits = checkpoint.checked_limits;
        if let Some(issue) = boundary_issue {
            self.issue_index
                .insert("nonstandard_record", self.issues.len());
            self.issues.push(issue);
        }
    }

    fn refresh_boundary_counts(&mut self) {
        if let Some(index) = self.issue_index.get("nonstandard_record").copied() {
            let issue = &mut self.issues[index];
            issue.count = 1;
            issue.affected_records = self.nonstandard_count;
        }
    }

    fn refresh_duplicate_summaries(&mut self) {
        for (code, label, total) in [
            ("duplicate_far", "FAR", self.far_count),
            ("duplicate_mir", "MIR", self.mir_count),
            ("duplicate_mrr", "MRR", self.mrr_count),
        ] {
            let Some(index) = self.issue_index.get(code).copied() else {
                continue;
            };
            let issue = &mut self.issues[index];
            issue.count = total.saturating_sub(1);
            issue.actual = format!(
                "文件中共读到 {total} 条 {label}，其中 {} 条是重复记录。",
                issue.count
            );
        }
    }

    fn promote_boundary_issue(&mut self, final_count: bool) {
        let Some(index) = self.issue_index.get("nonstandard_record").copied() else {
            return;
        };
        if let Some(evidence) = self.boundary_evidence.clone() {
            let pair_count = self.nonstandard_pairs.len();
            let raw_header = evidence
                .apparent_header
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<Vec<_>>()
                .join(" ");
            let spill = &evidence.apparent_header[..evidence.boundary_shift];
            let spill_hex = spill
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<Vec<_>>()
                .join(" ");
            let issue = &mut self.issues[index];
            issue.title = format!(
                "上一条 {} 的长度少了 {} 字节，导致记录边界错位",
                evidence.previous.record_type, evidence.boundary_shift
            );
            if let Some(field) = &evidence.field {
                let spill_ascii = String::from_utf8_lossy(spill);
                issue.actual = format!(
                    "上一条 {}（byte {}）写的是 REC_LEN={}、{} 长度={}。边界提前后，byte {} 的原始字节 {raw_header} 被当成记录头，得到 REC_LEN={}、REC_TYP={}、REC_SUB={}。",
                    evidence.previous.record_type,
                    evidence.previous.offset,
                    evidence.previous.length,
                    field.field_name,
                    field.declared_len,
                    evidence.unknown_offset,
                    evidence.apparent_len,
                    evidence.apparent_typ,
                    evidence.apparent_sub
                );
                issue.expected = format!(
                    "{} 应为 {} 字节：“{}”；后面的长度值 {} 表示 {} 是“{}”。因此上一条 {} 的 REC_LEN 应为 {}；下一条记录应从 byte {} 开始，记录头为 REC_LEN={}、REC_TYP={}、REC_SUB={}（{}）。",
                    field.field_name,
                    field.expected_len,
                    field.text,
                    field.following_text.len(),
                    field.following_field_name,
                    field.following_text,
                    evidence.previous.record_type,
                    evidence.previous_expected_len,
                    evidence.next_offset,
                    evidence.next_len,
                    evidence.next_typ,
                    evidence.next_sub,
                    evidence.next_name
                );
                issue.message = format!(
                    "REC_LEN={} 是把 {}“{}”末尾的字节 {}（ASCII“{}”）当成长度后得到的 0x{:04X}，并不是真正的记录长度。",
                    evidence.apparent_len,
                    field.following_field_name,
                    field.following_text,
                    spill_hex,
                    spill_ascii,
                    evidence.apparent_len
                );
                issue.suggestion = format!(
                    "请让文件生成方检查 byte {} 的 {}：REC_LEN 应从 {} 改为 {}，{} 长度应从 {} 改为 {}，然后重新导出原始 STDF。",
                    evidence.previous.offset,
                    evidence.previous.record_type,
                    evidence.previous.length,
                    evidence.previous_expected_len,
                    field.field_name,
                    field.declared_len,
                    field.expected_len
                );
            } else {
                issue.actual = format!(
                    "上一条 {}（byte {}）写的是 REC_LEN={}。边界提前后，byte {} 的原始字节 {raw_header} 被当成记录头，得到 REC_LEN={}、REC_TYP={}、REC_SUB={}。",
                    evidence.previous.record_type,
                    evidence.previous.offset,
                    evidence.previous.length,
                    evidence.unknown_offset,
                    evidence.apparent_len,
                    evidence.apparent_typ,
                    evidence.apparent_sub
                );
                issue.expected = format!(
                    "上一条 {} 的 REC_LEN 应为 {}；下一条记录应从 byte {} 开始，记录头为 REC_LEN={}、REC_TYP={}、REC_SUB={}（{}）。现有证据只能确定记录边界，无法可靠判断上一条记录中具体哪个字段写错。",
                    evidence.previous.record_type,
                    evidence.previous_expected_len,
                    evidence.next_offset,
                    evidence.next_len,
                    evidence.next_typ,
                    evidence.next_sub,
                    evidence.next_name
                );
                issue.message = format!(
                    "byte {} 开头的 {} 个字节实际属于上一条记录，却被和后面的记录头拼在一起，因此产生了假的 REC_LEN={}。",
                    evidence.unknown_offset, evidence.boundary_shift, evidence.apparent_len
                );
                issue.suggestion = format!(
                    "请让文件生成方检查 byte {} 的 {} 末尾字段和 REC_LEN；该记录至少应比当前声明多 {} 字节。",
                    evidence.previous.offset,
                    evidence.previous.record_type,
                    evidence.boundary_shift
                );
            }
            if final_count {
                issue.message.push_str(&format!(
                    "错位后共影响 {} 条无法识别的记录，涉及 {pair_count} 种 REC_TYP/REC_SUB 组合，相关统计可能不准确。",
                    self.nonstandard_count
                ));
            } else {
                issue
                    .message
                    .push_str("软件会继续按文件中写入的长度解析，但后续记录和统计可能不准确。");
            }
            let record_index = issue.samples.first().and_then(|sample| sample.record_index);
            issue.samples.clear();
            issue.samples.push(FileIssueLocation {
                offset: evidence.unknown_offset,
                record_index,
                record_type: "记录边界".to_string(),
                detail: format!(
                    "原始字节 {raw_header} 被误读为 REC_LEN={}、REC_TYP={}、REC_SUB={}；正确边界在 byte {}，记录头为 {:02X} {:02X} {:02X} {:02X}（REC_LEN={}，{}）。上一条 {} 的 REC_LEN 应为 {}。",
                    evidence.apparent_len,
                    evidence.apparent_typ,
                    evidence.apparent_sub,
                    evidence.next_offset,
                    evidence.next_len.to_le_bytes()[0],
                    evidence.next_len.to_le_bytes()[1],
                    evidence.next_typ,
                    evidence.next_sub,
                    evidence.next_len,
                    evidence.next_name,
                    evidence.previous.record_type,
                    evidence.previous_expected_len
                ),
            });
            issue.severity = FileIssueSeverity::Error;
            issue.count = 1;
            issue.affected_records = self.nonstandard_count;
            issue.affects_accuracy = true;
            return;
        }
        let issue = &mut self.issues[index];
        issue.title = "大量内容无法识别，文件边界可能已经错位".to_string();
        issue.actual = if final_count {
            format!(
                "共读到 {} 条无法识别的记录，涉及 {} 种不同的 REC_TYP/REC_SUB 组合。",
                self.nonstandard_count,
                self.nonstandard_pairs.len()
            )
        } else {
            format!(
                "已连续读到至少 {} 条无法识别的记录，涉及至少 {} 种不同类型。",
                self.nonstandard_count,
                self.nonstandard_pairs.len()
            )
        };
        issue.expected = "正常 STDF 数据应持续落在可识别的记录边界上；仅凭错位后的字节无法可靠反推出前一条记录正确的 REC_LEN。".to_string();
        issue.message = if final_count {
            "大量不同类型连续出现，通常不是单一厂商扩展，而是更早的 REC_LEN 或字段长度错误导致后续内容被误当成记录头。软件已按文件声明继续解析，但相关统计可能不准确。".to_string()
        } else {
            "大量不同类型连续出现，通常表示更早的 REC_LEN 或字段长度错误已经破坏记录边界。软件仍会按文件声明继续解析，但相关统计可能不准确。".to_string()
        };
        issue.severity = FileIssueSeverity::Error;
        issue.count = 1;
        issue.affected_records = self.nonstandard_count;
        issue.affects_accuracy = true;
    }

    fn boundary_untrusted(&self) -> bool {
        self.boundary_evidence.is_some()
            || (self.nonstandard_count >= 100 && self.nonstandard_pairs.len() >= 4)
    }

    fn infer_boundary_evidence(&self, record: &ParsedRecord) -> Option<BoundaryEvidence> {
        let previous = self.previous?;
        if previous.record_type == "UNKNOWN" {
            return None;
        }
        let apparent_header = [
            record.length.to_le_bytes()[0],
            record.length.to_le_bytes()[1],
            record.rec_typ,
            record.rec_sub,
        ];
        let mut stream = Vec::with_capacity(4 + record.diagnostic_payload_prefix.len());
        stream.extend_from_slice(&apparent_header);
        stream.extend_from_slice(&record.diagnostic_payload_prefix);
        let mut candidates = Vec::new();
        for shift in 1..=3 {
            let Some(candidate) = stream.get(shift..shift + 4) else {
                continue;
            };
            let next_len = u16::from_le_bytes([candidate[0], candidate[1]]);
            let next_typ = candidate[2];
            let next_sub = candidate[3];
            let next_name = record_name(next_typ, next_sub);
            if next_name == "UNKNOWN" {
                continue;
            }
            let following_header_start = shift + 4 + usize::from(next_len);
            let Some(following_header) =
                stream.get(following_header_start..following_header_start + 4)
            else {
                continue;
            };
            if record_name(following_header[2], following_header[3]) != "UNKNOWN" {
                candidates.push((shift, next_len, next_typ, next_sub, next_name));
            }
        }
        if candidates.len() != 1 {
            return None;
        }
        let (missing, next_len, next_typ, next_sub, next_name) = candidates[0];
        let previous_expected_len = usize::from(previous.length) + missing;
        let field = self.previous_text_split.as_ref().and_then(|text| {
            let text_missing = text
                .following_len
                .checked_sub(text.following_prefix.len())?;
            if text_missing != missing
                || text
                    .following_prefix
                    .iter()
                    .chain(apparent_header[..missing].iter())
                    .any(|byte| !byte.is_ascii_graphic() && *byte != b' ')
            {
                return None;
            }
            let reconstructed_len =
                text.field_offset + 1 + text.expected_len + 1 + text.following_len;
            if reconstructed_len != previous_expected_len {
                return None;
            }
            let mut following_bytes = text.following_prefix.clone();
            following_bytes.extend_from_slice(&apparent_header[..missing]);
            Some(BoundaryFieldEvidence {
                field_name: text.field_name.clone(),
                following_field_name: text.following_field_name.clone(),
                declared_len: text.declared_len,
                expected_len: text.expected_len,
                text: text.text.clone(),
                following_text: String::from_utf8(following_bytes).ok()?,
            })
        });
        Some(BoundaryEvidence {
            previous,
            previous_expected_len,
            field,
            unknown_offset: record.offset,
            apparent_header,
            apparent_len: record.length,
            apparent_typ: record.rec_typ,
            apparent_sub: record.rec_sub,
            boundary_shift: missing,
            next_offset: record.offset + missing as u64,
            next_len,
            next_typ,
            next_sub,
            next_name,
        })
    }
}

fn suspicious_text_split(record: &ParsedRecord) -> Option<SuspiciousTextSplit> {
    let payload_end = record.offset + 4 + u64::from(record.length);
    let field_index = record.fields.iter().rposition(|field| {
        field.offset.is_some() && field.length.is_some_and(|length| length > 0)
    })?;
    let field = record.fields.get(field_index)?;
    let following = record.fields.get(field_index + 1)?;
    if field.field_type.as_ref() != "C*n"
        || following.field_type.as_ref() != "C*n"
        || following.offset.is_some()
        || field.offset? + u64::from(field.length?) != payload_end
    {
        return None;
    }
    let bytes = field.value.as_bytes();
    let field_offset = field.offset?.checked_sub(record.offset + 4)? as usize;
    let declared_len = usize::from(field.length?).checked_sub(1)?;
    let mut candidates = Vec::new();
    for (split, marker) in bytes.iter().copied().enumerate() {
        let following_len = usize::from(marker);
        let following_prefix = &bytes[split + 1..];
        if marker == 0
            || marker > 31
            || following_prefix.len() >= following_len
            || following_len.saturating_sub(following_prefix.len()) > 3
            || !bytes[..split]
                .iter()
                .chain(following_prefix.iter())
                .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
        {
            continue;
        }
        candidates.push(SuspiciousTextSplit {
            field_name: field.name.to_string(),
            following_field_name: following.name.to_string(),
            declared_len,
            expected_len: split,
            field_offset,
            text: String::from_utf8(bytes[..split].to_vec()).ok()?,
            following_len,
            following_prefix: following_prefix.to_vec(),
        });
    }
    (candidates.len() == 1).then(|| candidates.remove(0))
}

fn location(
    record: &ParsedRecord,
    record_index: usize,
    detail: impl Into<String>,
) -> Option<FileIssueLocation> {
    Some(FileIssueLocation {
        offset: record.offset,
        record_index: Some(record_index),
        record_type: record.record_type.to_string(),
        detail: detail.into(),
    })
}

fn field_value<'a>(fields: &'a [ParsedField], name: &str) -> &'a str {
    fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| field.value.as_str())
        .unwrap_or("")
}

fn parse_u16(value: &str) -> Option<u16> {
    value.trim().parse().ok()
}

fn parse_u64(value: &str) -> Option<u64> {
    value.trim().parse().ok()
}

fn parse_usize(value: &str) -> Option<usize> {
    value.trim().parse().ok()
}

fn parse_b1(value: &str) -> Option<u8> {
    u8::from_str_radix(value.trim().strip_prefix("0b")?, 2).ok()
}

fn head_site(fields: &[ParsedField]) -> (u8, u8) {
    (
        field_value(fields, "HEAD_NUM").parse().unwrap_or(u8::MAX),
        field_value(fields, "SITE_NUM").parse().unwrap_or(u8::MAX),
    )
}

fn head_site_group(fields: &[ParsedField]) -> (u8, u8) {
    (
        field_value(fields, "HEAD_NUM").parse().unwrap_or(u8::MAX),
        field_value(fields, "SITE_GRP").parse().unwrap_or(u8::MAX),
    )
}

fn array_preview_values<'a>(fields: &'a [ParsedField], name: &str) -> Vec<&'a str> {
    let raw = field_value(fields, name);
    let Some(start) = raw.find('[') else {
        return Vec::new();
    };
    let Some(end) = raw.rfind(']') else {
        return Vec::new();
    };
    if end <= start + 1 {
        return Vec::new();
    }
    raw[start + 1..end]
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "...")
        .collect()
}

fn scaled_value(
    fields: &[ParsedField],
    value_name: &str,
    scale_name: &str,
    default_scale: i32,
) -> Option<f64> {
    let value = field_value(fields, value_name).parse::<f64>().ok()?;
    let scale = field_value(fields, scale_name)
        .parse::<i32>()
        .unwrap_or(default_scale);
    let scaled = value * 10_f64.powi(-scale);
    scaled.is_finite().then_some(scaled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{parse_reader, RecordParseIssueKind};
    use std::fs::File;
    use std::io::Cursor;

    fn record(rec_typ: u8, rec_sub: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        bytes.push(rec_typ);
        bytes.push(rec_sub);
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn collector_keeps_valid_minimal_file_clean() {
        let mut bytes = record(0, 10, &[2, 4]);
        let mut mir = Vec::new();
        mir.extend_from_slice(&0_u32.to_le_bytes());
        mir.extend_from_slice(&1_u32.to_le_bytes());
        mir.extend_from_slice(&[0, b'P', b' ', b' ']);
        mir.extend_from_slice(&0_u16.to_le_bytes());
        mir.push(b' ');
        // Required C*n fields may be empty, but each still needs its length byte.
        mir.extend_from_slice(&[0, 0, 0, 0, 0]);
        bytes.extend(record(1, 10, &mir));
        bytes.extend(record(1, 20, &2_u32.to_le_bytes()));
        let mut collector = FileIssueCollector::default();
        let mut index = 0;
        let result = parse_reader(
            &mut Cursor::new(bytes.clone()),
            bytes.len() as u64,
            |parsed| {
                collector.observe(&parsed, index);
                index += 1;
                true
            },
            |_, _| {},
        );
        collector.finish(result.as_ref().err());
        assert!(collector.issues().is_empty(), "{:?}", collector.issues());
    }

    #[test]
    fn collector_aggregates_unknown_records_and_keeps_first_location() {
        let mut collector = FileIssueCollector::default();
        for index in 0..120 {
            let mut parsed = Vec::new();
            let bytes = record(99, (index % 8) as u8, &[1, 2, 3]);
            parse_reader(
                &mut Cursor::new(bytes.clone()),
                bytes.len() as u64,
                |record| {
                    parsed.push(record);
                    true
                },
                |_, _| {},
            )
            .unwrap();
            collector.observe(&parsed[0], index);
        }
        collector.finish(None);
        let issue = collector
            .issues()
            .iter()
            .find(|issue| issue.code == "nonstandard_record")
            .unwrap();
        assert_eq!(issue.count, 1);
        assert_eq!(issue.affected_records, 120);
        assert_eq!(issue.samples.len(), 3);
        assert_eq!(issue.severity, FileIssueSeverity::Error);
    }

    #[test]
    fn repeated_single_vendor_record_type_stays_a_warning() {
        let mut collector = FileIssueCollector::default();
        for index in 0..120 {
            let mut parsed = Vec::new();
            let bytes = record(99, 1, &[1, 2, 3]);
            parse_reader(
                &mut Cursor::new(bytes.clone()),
                bytes.len() as u64,
                |record| {
                    parsed.push(record);
                    true
                },
                |_, _| {},
            )
            .unwrap();
            collector.observe(&parsed[0], index);
        }
        collector.finish(None);

        let issue = collector
            .issues()
            .iter()
            .find(|issue| issue.code == "nonstandard_record")
            .unwrap();
        assert_eq!(issue.count, 120);
        assert_eq!(issue.severity, FileIssueSeverity::Warning);
    }

    #[test]
    fn explains_a_ptr_boundary_shift_when_the_next_headers_prove_it() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&100_u32.to_le_bytes());
        payload.extend_from_slice(&[1, 3, 0, 0]);
        payload.extend_from_slice(&1.0_f32.to_le_bytes());
        payload.push(37);
        payload.extend_from_slice(b"FT1_func_crgclk_test_PLL_LOCK_MRC");
        payload.extend_from_slice(&[5, b'%', b'5', b'.']);
        assert_eq!(payload.len(), 50);

        let mut previous = None;
        parse_reader(
            &mut Cursor::new(record(15, 10, &payload)),
            54,
            |parsed| {
                previous = Some(parsed);
                true
            },
            |_, _| {},
        )
        .unwrap();

        // The apparent UNKNOWN header begins with the two bytes still belonging
        // to "%5.3f". Its payload then begins with the last two bytes of the
        // real PTR header and contains another valid PTR header 54 bytes later.
        let mut diagnostic_prefix = vec![15, 10];
        diagnostic_prefix.extend_from_slice(&[0; 50]);
        diagnostic_prefix.extend_from_slice(&[50, 0, 15, 10]);
        let unknown = ParsedRecord {
            record_type: "UNKNOWN",
            rec_typ: 50,
            rec_sub: 0,
            offset: 54,
            length: 26_163,
            fields: Vec::new(),
            status: RecordStatus::Unknown,
            mpr_results: None,
            parse_issue: None,
            diagnostic_payload_prefix: diagnostic_prefix,
        };

        let mut collector = FileIssueCollector::default();
        collector.open_parts.insert(
            (1, 3),
            OpenPart {
                offset: 12,
                record_index: 0,
                test_count: 1,
            },
        );
        collector.open_bps.push((24, 1));
        collector.observe(&previous.unwrap(), 0);
        collector.observe(&unknown, 1);
        collector.finish(None);

        let issue = collector
            .issues()
            .iter()
            .find(|issue| issue.code == "nonstandard_record")
            .unwrap();
        assert_eq!(
            issue.title,
            "上一条 PTR 的长度少了 2 字节，导致记录边界错位"
        );
        assert!(
            issue.actual.contains("REC_LEN=50、TEST_TXT 长度=37"),
            "{issue:#?}"
        );
        assert!(issue
            .actual
            .contains("REC_LEN=26163、REC_TYP=50、REC_SUB=0"));
        assert!(issue.expected.contains("TEST_TXT 应为 33 字节"));
        assert!(issue.expected.contains("REC_LEN 应为 52"));
        assert!(issue
            .expected
            .contains("REC_LEN=50、REC_TYP=15、REC_SUB=10（PTR）"));
        assert!(issue.message.contains("0x6633"));
        assert!(issue.message.contains("ASCII“3f”"));
        assert_eq!(issue.count, 1);
        assert_eq!(issue.affected_records, 1);
        assert_eq!(issue.samples.len(), 1);
        assert_eq!(issue.samples[0].record_type, "记录边界");
        assert!(issue.samples[0].detail.contains("正确边界在 byte 56"));
        assert!(!collector
            .issues()
            .iter()
            .any(|item| matches!(item.code.as_str(), "unclosed_parts" | "unclosed_bps")));
    }

    #[test]
    fn boundary_field_evidence_is_not_limited_to_ptr_records() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&200_u32.to_le_bytes());
        payload.extend_from_slice(&[1, 2, 0, 0]);
        payload.extend_from_slice(&0_u16.to_le_bytes());
        payload.extend_from_slice(&0_u16.to_le_bytes());
        payload.push(12);
        payload.extend_from_slice(b"MPR_TEXT");
        payload.extend_from_slice(&[5, b'a', b'b', b'c']);
        assert_eq!(payload.len(), 25);

        let mut previous = None;
        parse_reader(
            &mut Cursor::new(record(15, 15, &payload)),
            29,
            |parsed| {
                previous = Some(parsed);
                true
            },
            |_, _| {},
        )
        .unwrap();
        assert!(
            suspicious_text_split(previous.as_ref().unwrap()).is_some(),
            "{:#?}",
            previous.as_ref().unwrap().fields
        );

        let unknown = ParsedRecord {
            record_type: "UNKNOWN",
            rec_typ: 2,
            rec_sub: 0,
            offset: 29,
            length: u16::from_le_bytes(*b"de"),
            fields: Vec::new(),
            status: RecordStatus::Unknown,
            mpr_results: None,
            parse_issue: None,
            diagnostic_payload_prefix: vec![0, 10, 2, 4, 4, 0, 1, 20],
        };
        let mut collector = FileIssueCollector::default();
        collector.observe(&previous.unwrap(), 0);
        collector.observe(&unknown, 1);
        collector.finish(None);

        let issue = collector
            .issues()
            .iter()
            .find(|issue| issue.code == "nonstandard_record")
            .unwrap();
        assert!(issue.actual.contains("上一条 MPR"));
        assert!(issue.actual.contains("TEST_TXT 长度=12"), "{issue:#?}");
        assert!(issue.expected.contains("TEST_TXT 应为 8 字节"));
        assert!(issue.expected.contains("ALARM_ID 是“abcde”"));
        assert!(issue.expected.contains("REC_LEN 应为 27"));
    }

    #[test]
    fn heterogeneous_issue_groups_label_the_top_values_as_the_first_example() {
        let mut collector = FileIssueCollector::default();
        for (index, (field_name, expected_bytes)) in [("STDF_VER", 1_usize), ("TEST_NUM", 4_usize)]
            .into_iter()
            .enumerate()
        {
            let record = ParsedRecord {
                record_type: "FAR",
                rec_typ: 0,
                rec_sub: 10,
                offset: index as u64 * 10,
                length: 1,
                fields: Vec::new(),
                status: RecordStatus::Error,
                mpr_results: None,
                parse_issue: Some(crate::parser::RecordParseIssue {
                    kind: RecordParseIssueKind::RequiredFieldMissing,
                    field_name,
                    expected_bytes,
                    remaining_bytes: 0,
                }),
                diagnostic_payload_prefix: Vec::new(),
            };
            collector.observe(&record, index);
        }
        collector.finish(None);
        let issue = collector
            .issues()
            .iter()
            .find(|issue| issue.code == "record_field_incomplete")
            .unwrap();
        assert_eq!(issue.count, 2);
        assert_eq!(issue.title, "多条记录的字段结构不完整");
        assert!(issue.actual.starts_with("不同位置的实际值不完全相同"));
        assert!(issue.expected.starts_with("每一处都应满足对应规则"));
        assert_eq!(issue.samples.len(), 2);
    }

    #[test]
    fn boundary_loss_is_promoted_live_and_suppresses_later_cascade_issues() {
        let mut collector = FileIssueCollector::default();
        for index in 0..100 {
            let mut parsed = Vec::new();
            let bytes = record(99, (index % 4) as u8, &[1, 2, 3]);
            parse_reader(
                &mut Cursor::new(bytes.clone()),
                bytes.len() as u64,
                |record| {
                    parsed.push(record);
                    true
                },
                |_, _| {},
            )
            .unwrap();
            collector.observe(&parsed[0], index);
        }

        let malformed = ParsedRecord {
            record_type: "PTR",
            rec_typ: 15,
            rec_sub: 10,
            offset: 1_000,
            length: 1,
            fields: Vec::new(),
            status: RecordStatus::Error,
            mpr_results: None,
            parse_issue: Some(crate::parser::RecordParseIssue {
                kind: RecordParseIssueKind::RequiredFieldMissing,
                field_name: "TEST_NUM",
                expected_bytes: 4,
                remaining_bytes: 0,
            }),
            diagnostic_payload_prefix: Vec::new(),
        };
        collector.observe(&malformed, 100);

        let boundary = collector
            .issues()
            .iter()
            .find(|issue| issue.code == "nonstandard_record")
            .unwrap();
        assert_eq!(boundary.severity, FileIssueSeverity::Error);
        assert!(boundary.actual.contains("100 条无法识别"));
        assert!(boundary.expected.contains("无法可靠反推出"));
        assert!(!collector
            .issues()
            .iter()
            .any(|issue| issue.code == "record_field_incomplete"));
    }

    #[test]
    fn truncated_tail_is_marked_as_affecting_accuracy() {
        let mut collector = FileIssueCollector::default();
        collector.finish(Some(&ParserError::TruncatedHeader {
            offset: 12,
            available: 2,
        }));

        let issue = collector
            .issues()
            .iter()
            .find(|issue| issue.code == "truncated_header")
            .unwrap();
        assert!(issue.affects_accuracy);
    }

    #[test]
    fn parser_diagnostic_is_rendered_as_field_issue() {
        let record = ParsedRecord {
            record_type: "FAR",
            rec_typ: 0,
            rec_sub: 10,
            offset: 42,
            length: 1,
            fields: Vec::new(),
            status: RecordStatus::Error,
            mpr_results: None,
            parse_issue: Some(crate::parser::RecordParseIssue {
                kind: RecordParseIssueKind::RequiredFieldMissing,
                field_name: "STDF_VER",
                expected_bytes: 1,
                remaining_bytes: 0,
            }),
            diagnostic_payload_prefix: Vec::new(),
        };
        let mut collector = FileIssueCollector::default();
        collector.observe(&record, 0);
        collector.finish(None);
        let issue = collector
            .issues()
            .iter()
            .find(|issue| issue.code == "record_field_incomplete")
            .unwrap();
        assert!(issue.actual.contains("REC_LEN=1"));
        assert!(issue.actual.contains("STDF_VER"));
        assert!(issue.expected.contains("REC_LEN 至少应为 2"));
        assert!(issue.message.contains("少 1 字节"));
    }

    #[test]
    fn every_diagnostic_exposes_a_plain_language_comparison() {
        let mut collector = FileIssueCollector::default();
        collector.finish(Some(&ParserError::TruncatedPayload {
            offset: 12,
            expected: 50,
            available: 46,
            rec_typ: 15,
            rec_sub: 10,
        }));

        for issue in collector.issues() {
            assert!(!issue.actual.trim().is_empty(), "{} actual", issue.code);
            assert!(!issue.expected.trim().is_empty(), "{} expected", issue.code);
            assert!(!issue.message.trim().is_empty(), "{} message", issue.code);
        }
        let truncated = collector
            .issues()
            .iter()
            .find(|issue| issue.code == "truncated_payload")
            .unwrap();
        assert!(truncated.actual.contains("REC_LEN=50"));
        assert!(truncated.actual.contains("46 字节"));
        assert!(truncated.message.contains("少了 4 字节"));
    }

    #[test]
    #[ignore = "requires STDF_SAMPLE_PATH pointing at a local sample"]
    fn collector_scans_configured_sample() {
        let path = std::env::var("STDF_SAMPLE_PATH").expect("set STDF_SAMPLE_PATH");
        let mut file = File::open(&path).expect("open sample");
        let total = file.metadata().expect("sample metadata").len();
        let mut collector = FileIssueCollector::default();
        let mut index = 0_usize;
        let result = parse_reader(
            &mut file,
            total,
            |record| {
                collector.observe(&record, index);
                index += 1;
                true
            },
            |_, _| {},
        );
        collector.finish(result.as_ref().err());
        eprintln!("parsed {index} records; issues: {:#?}", collector.issues());
        assert!(index > 0);
    }
}
