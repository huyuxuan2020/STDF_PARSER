use crate::parser::{ParsedField, ParsedRecord, ParserError, RecordParseIssueKind, RecordStatus};
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
    pub message: String,
    pub suggestion: String,
    pub count: usize,
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
}

impl FileIssueCollector {
    pub fn observe(&mut self, record: &ParsedRecord, record_index: usize) {
        self.record_count += 1;

        if record_index == 0 && record.record_type != "FAR" {
            self.push(
                "missing_initial_far",
                FileIssueSeverity::Error,
                "文件没有从标准起点开始",
                "STDF 文件通常应以 FAR 记录开头。当前文件的第一条记录不是 FAR，文件可能缺少开头、被截取，或并非完整的 STDF。",
                "建议重新导出完整文件，并确认传输过程中没有被截断。",
                true,
                location(record, record_index, "这是软件读到的第一条记录。"),
            );
        }

        if !self.boundary_untrusted() {
            if let Some(issue) = &record.parse_issue {
                let (title, message) = match issue.kind {
                RecordParseIssueKind::RequiredFieldMissing => (
                    "记录缺少必填内容",
                    format!(
                        "{} 记录中的 {} 字段没有完整写入。这条记录的内容不完整，但软件会继续读取后面的记录。",
                        record.record_type, issue.field_name
                    ),
                ),
                RecordParseIssueKind::FixedFieldTruncated => (
                    "字段在记录中途结束",
                    format!(
                        "{} 记录中的 {} 字段需要 {} 字节，但记录中只剩 {} 字节。",
                        record.record_type,
                        issue.field_name,
                        issue.expected_bytes,
                        issue.remaining_bytes
                    ),
                ),
                RecordParseIssueKind::DeclaredLengthExceedsRecord => (
                    "字段声明的长度超出记录范围",
                    format!(
                        "{} 记录中的 {} 字段声明需要 {} 字节，但当前记录只剩 {} 字节。",
                        record.record_type,
                        issue.field_name,
                        issue.expected_bytes,
                        issue.remaining_bytes
                    ),
                ),
                RecordParseIssueKind::ArrayExceedsRecord => (
                    "数组内容没有完整写入",
                    format!(
                        "{} 记录中的 {} 数组应占 {} 字节，但当前记录只剩 {} 字节。",
                        record.record_type,
                        issue.field_name,
                        issue.expected_bytes,
                        issue.remaining_bytes
                    ),
                ),
                RecordParseIssueKind::UnexpectedTrailingBytes => (
                    "记录末尾存在无法解释的内容",
                    format!(
                        "{} 的标准字段读取完成后，记录末尾仍有 {} 字节无法对应到标准字段。它可能来自厂商扩展，也可能是长度写入异常。",
                        record.record_type, issue.remaining_bytes
                    ),
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
            self.nonstandard_count += 1;
            self.nonstandard_pairs
                .insert((record.rec_typ, record.rec_sub));
            self.first_unknown_offset.get_or_insert(record.offset);
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
                "文件中出现了 STDF V4 标准表之外的 record 类型。它可能是厂商扩展；如果数量很多，通常表示前面的记录长度已经写错，后续内容被误当成 record。",
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
        }

        if self.boundary_untrusted() {
            if !self.boundary_collapsed {
                self.collapse_cascade_issues();
                self.promote_boundary_issue(false);
                self.boundary_collapsed = true;
            }
            self.remember(record);
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
                        "MIR 是整份测试文件的主信息记录，通常只应出现一次。多条 MIR 可能表示多个文件被拼接在一起。",
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
                        "MRR 表示一次测试数据流结束。出现多条 MRR，可能意味着多个 STDF 内容被拼接。",
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
                        "软件读到了 EPS，但此前没有尚未结束的 BPS。测试段的开始或结束记录可能缺失。",
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
                "MRR 表示测试数据已经结束，但文件在 MRR 后面仍包含其他 records。文件可能被拼接，或结束记录写入位置不正确。",
                "建议确认 MRR 后面的内容是否属于另一份测试数据。",
                false,
                location(record, record_index, "这条记录出现在 MRR 之后。"),
            );
        }

        self.remember(record);
    }

    fn remember(&mut self, record: &ParsedRecord) {
        self.previous = Some(RecordMeta {
            offset: record.offset,
            length: record.length,
            rec_typ: record.rec_typ,
            rec_sub: record.rec_sub,
            record_type: record.record_type,
        });
    }

    pub fn finish(&mut self, parser_error: Option<&ParserError>) {
        if self.record_count == 0 {
            self.push_without_location(
                "empty_file",
                FileIssueSeverity::Error,
                "文件中没有可读取的记录",
                "软件没有从文件中读到任何完整的 STDF record。文件可能为空、格式不正确或只包含残缺数据。",
                "请确认选择的是原始 STDF 文件，并尝试重新导出或重新传输。",
                true,
            );
        }
        if self.far_count == 0 && self.record_count > 0 {
            self.push_without_location(
                "missing_far",
                FileIssueSeverity::Error,
                "文件缺少 FAR 基本信息",
                "整份文件中没有找到 FAR，因此无法确认 STDF 版本和数据字节序。",
                "建议让文件生成方重新导出完整 STDF。",
                true,
            );
        }
        if self.mir_count == 0 && self.record_count > 0 {
            self.push_without_location(
                "missing_mir",
                FileIssueSeverity::Warning,
                "文件缺少测试主信息",
                "没有找到 MIR，因此批次、产品、测试程序和开始时间等关键信息可能无法显示。",
                "建议确认文件是否从测试中途截取。",
                false,
            );
        }
        if self.mrr_count == 0 && self.record_count > 0 {
            self.push_without_location(
                "missing_mrr",
                FileIssueSeverity::Warning,
                "文件缺少正常结束标记",
                "没有找到 MRR。文件可能在测试结束前被截断，或生成程序没有写入结束记录。",
                "建议确认测试是否完整结束，并重新导出文件。",
                false,
            );
        }

        if !self.open_parts.is_empty() {
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
                format!("解析结束时仍有 {count} 个 PIR 没有找到对应的 PRR。"),
                "这些器件的 bin、测试耗时和器件编号可能不完整。",
                false,
                first.map(|item| FileIssueLocation {
                    offset: item.offset,
                    record_index: Some(item.record_index),
                    record_type: "PIR".to_string(),
                    detail: "这是最早尚未配对的 PIR。".to_string(),
                }),
            );
        }
        if !self.open_wafers.is_empty() {
            let first = self.open_wafers.values().min_by_key(|item| item.0).copied();
            self.push(
                "unclosed_wafers",
                FileIssueSeverity::Warning,
                "有晶圆记录没有正常结束",
                format!(
                    "解析结束时仍有 {} 条 WIR 没有对应的 WRR。",
                    self.open_wafers.len()
                ),
                "晶圆汇总数量和结束时间可能不完整。",
                false,
                first.map(|item| FileIssueLocation {
                    offset: item.0,
                    record_index: Some(item.1),
                    record_type: "WIR".to_string(),
                    detail: "这是最早尚未配对的 WIR。".to_string(),
                }),
            );
        }
        if !self.open_bps.is_empty() {
            let first = self.open_bps.first().copied();
            self.push(
                "unclosed_bps",
                FileIssueSeverity::Warning,
                "有测试段没有正常结束",
                format!(
                    "解析结束时仍有 {} 条 BPS 没有对应的 EPS。",
                    self.open_bps.len()
                ),
                "测试段名称和范围可能不完整。",
                false,
                first.map(|item| FileIssueLocation {
                    offset: item.0,
                    record_index: Some(item.1),
                    record_type: "BPS".to_string(),
                    detail: "这是最早尚未配对的 BPS。".to_string(),
                }),
            );
        }

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
                "MPR 或 FTR 中使用了找不到对应 PMR 定义的 pin 索引，因此部分 pin 名称无法解析。",
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
                    "MIR 中的开始时间晚于 MRR 中的结束时间，文件时间信息可能不正确。",
                    "建议检查测试机时间设置和文件生成时间。",
                    false,
                );
            }
        }

        if let Some(error) = parser_error {
            self.observe_parser_error(error);
        }

        let boundary_untrusted = self.boundary_untrusted();
        if boundary_untrusted {
            self.promote_boundary_issue(true);
        }

        // Once many non-standard headers appear after one point, later
        // record-level checks are no longer independent evidence: random
        // payload bytes can resemble PTR/PIR/etc. Keep the first boundary
        // problem and any issues proven before it, but collapse downstream
        // consequences into the single nonstandard_record summary.
        if boundary_untrusted {
            self.collapse_cascade_issues();
            self.boundary_collapsed = true;
        }
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
                "FAR 应标记一份 STDF 数据的开始。出现多个 FAR，通常表示多个文件被直接拼接。",
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
                format!("FAR 中的 CPU_TYPE 为 {cpu}。当前版本仅可靠支持常见的 little-endian STDF V4 文件。"),
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
                format!("文件声明的 STDF 版本为 {version}，当前版本只按 STDF V4/V4-2007 解析。"),
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
                "同一 head/site 的上一颗器件尚未出现 PRR，就又出现了新的 PIR。上一颗器件的结束信息可能缺失。",
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
                "软件读到了 PTR/MPR/FTR，但同一 head/site 此前没有 PIR。该测试结果无法可靠归属到具体器件。",
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
                "软件读到了 PRR，但同一 head/site 此前没有 PIR。器件测试过程可能不完整。",
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
                        "PRR 声明这颗器件执行了 {declared} 项测试，但在对应 PIR/PRR 之间实际读到 {} 条 PTR/MPR/FTR。",
                        part.test_count
                    ),
                    "这可能来自测试机的计数口径差异；若差异普遍存在，建议检查文件生成逻辑。",
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
                "上一条 WIR 尚未由 WRR 结束，就出现了新的 WIR。晶圆边界可能不完整。",
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
                "软件读到了 WRR，但同一 head/site group 此前没有 WIR。",
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
                format!("PMR 索引 {index} 在同一 head/site 中重复出现，后续记录引用该索引时可能无法确定对应哪个 pin。"),
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
                        "{} 的 {} 字段是 {value}，良率判断、范围计算或导出结果可能受到影响。",
                        record.record_type, field.name
                    ),
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
                    "{} 测试号 {test_num} 的低限大于高限，自动判定结果可能不符合预期。",
                    record.record_type
                ),
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
                format!("最后一个 record header 应有 4 字节，但文件在 byte {offset} 处只剩 {available} 字节。"),
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
                format!("最后一条记录声明有 {expected} 字节内容，但文件中实际只有 {available} 字节。此前完整记录仍可查看。"),
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
                "文件看起来使用 big-endian STDF 格式，当前版本无法可靠解释其中的数值。",
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
                format!("解析过程中系统无法继续读取文件：{error}"),
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
        message: impl Into<String>,
        suggestion: impl Into<String>,
        affects_accuracy: bool,
        sample: Option<FileIssueLocation>,
    ) {
        if let Some(index) = self.issue_index.get(code).copied() {
            let issue = &mut self.issues[index];
            issue.count += 1;
            issue.affects_accuracy |= affects_accuracy;
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
            message: message.into(),
            suggestion: suggestion.into(),
            count: 1,
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
        message: impl Into<String>,
        suggestion: impl Into<String>,
        affects_accuracy: bool,
    ) {
        self.push(
            code,
            severity,
            title,
            message,
            suggestion,
            affects_accuracy,
            None,
        );
    }

    fn rebuild_issue_index(&mut self) {
        self.issue_index.clear();
        for (index, issue) in self.issues.iter().enumerate() {
            // All codes originate from static literals in this module. Match
            // them back to those literals instead of leaking owned strings.
            if let Some(code) = static_issue_code(&issue.code) {
                self.issue_index.insert(code, index);
            }
        }
    }

    fn collapse_cascade_issues(&mut self) {
        let Some(first_unknown) = self.first_unknown_offset else {
            return;
        };
        self.issues.retain(|issue| {
            if !cascade_prone(issue.code.as_str()) {
                return true;
            }
            issue
                .samples
                .iter()
                .any(|sample| sample.offset < first_unknown)
        });
        self.rebuild_issue_index();
    }

    fn promote_boundary_issue(&mut self, final_count: bool) {
        let Some(index) = self.issue_index.get("nonstandard_record").copied() else {
            return;
        };
        let issue = &mut self.issues[index];
        issue.title = "大量内容无法识别，文件边界可能已经错位".to_string();
        issue.message = if final_count {
            format!(
                "共发现 {} 条非标准或无法识别的记录。大量连续出现通常不是厂商扩展，而是较早的 REC_LEN 或字段长度错误导致后续内容被误当成 record。软件已按文件声明继续解析，但相关统计可能不准确。",
                issue.count
            )
        } else {
            "文件中已连续出现大量不同类型的无法识别记录。这通常不是厂商扩展，而是较早的 REC_LEN 或字段长度错误导致后续内容被误当成 record。软件仍会按文件声明继续解析，但相关统计可能不准确。".to_string()
        };
        issue.severity = FileIssueSeverity::Error;
        issue.affects_accuracy = true;
    }

    fn boundary_untrusted(&self) -> bool {
        self.nonstandard_count >= 100 && self.nonstandard_pairs.len() >= 4
    }
}

fn cascade_prone(code: &str) -> bool {
    matches!(
        code,
        "record_field_incomplete"
            | "unexpected_record_tail"
            | "duplicate_far"
            | "duplicate_mir"
            | "duplicate_mrr"
            | "records_after_mrr"
            | "missing_mrr"
            | "overlapping_pir"
            | "test_without_pir"
            | "prr_without_pir"
            | "part_test_count_mismatch"
            | "overlapping_wir"
            | "wrr_without_wir"
            | "eps_without_bps"
            | "unclosed_parts"
            | "unclosed_wafers"
            | "unclosed_bps"
            | "duplicate_pmr_index"
            | "missing_pmr_reference"
            | "non_finite_number"
            | "limit_order_invalid"
    )
}

fn static_issue_code(code: &str) -> Option<&'static str> {
    const CODES: &[&str] = &[
        "missing_initial_far",
        "record_field_incomplete",
        "unexpected_record_tail",
        "nonstandard_record",
        "duplicate_mir",
        "duplicate_mrr",
        "eps_without_bps",
        "records_after_mrr",
        "empty_file",
        "missing_far",
        "missing_mir",
        "missing_mrr",
        "unclosed_parts",
        "unclosed_wafers",
        "unclosed_bps",
        "missing_pmr_reference",
        "time_order_invalid",
        "duplicate_far",
        "unsupported_cpu_type",
        "unsupported_stdf_version",
        "overlapping_pir",
        "test_without_pir",
        "prr_without_pir",
        "part_test_count_mismatch",
        "overlapping_wir",
        "wrr_without_wir",
        "duplicate_pmr_index",
        "non_finite_number",
        "limit_order_invalid",
        "truncated_header",
        "truncated_payload",
        "unsupported_byte_order",
        "file_read_error",
    ];
    CODES.iter().copied().find(|item| *item == code)
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
        assert_eq!(issue.count, 120);
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
        };
        collector.observe(&malformed, 100);

        let boundary = collector
            .issues()
            .iter()
            .find(|issue| issue.code == "nonstandard_record")
            .unwrap();
        assert_eq!(boundary.severity, FileIssueSeverity::Error);
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
        };
        let mut collector = FileIssueCollector::default();
        collector.observe(&record, 0);
        collector.finish(None);
        assert!(collector
            .issues()
            .iter()
            .any(|issue| issue.code == "record_field_incomplete"));
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
