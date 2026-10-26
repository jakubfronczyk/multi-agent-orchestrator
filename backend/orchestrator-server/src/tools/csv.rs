//! `csv_query`: filter, select, count or group the rows of a workspace CSV file.
//! It reads through `Workspace`, so trust and taint follow the same rule as `file_read`.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use orchestrator_core::ids::TeamId;
use orchestrator_core::tools::{
    Action, Tool, ToolCtx, ToolError, ToolName, ToolOutput, ToolSpec, parse_args,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::files::{file_action, file_taint};
use super::spec;
use super::workspace::{Need, Workspace};

const TIMEOUT: Duration = Duration::from_secs(5);
const ROW_LIMIT: usize = 200;

#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum FilterOp {
    Eq,
    Ne,
    Contains,
    Gt,
    Lt,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Filter {
    /// A column name from the header row.
    column: String,
    /// `eq`, `ne`, `contains`, `gt` or `lt`; `gt` and `lt` compare numbers when both sides are numbers.
    op: FilterOp,
    value: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CsvArgs {
    /// A .csv file inside the workspace.
    path: String,
    /// Rows must match every filter.
    #[serde(default)]
    filter: Vec<Filter>,
    /// Columns to return; all columns when absent or empty.
    #[serde(default)]
    select: Vec<String>,
    /// Count matching rows per value of this column; empty means no grouping.
    #[serde(default)]
    group_by: Option<String>,
    /// Return only the number of matching rows.
    #[serde(default)]
    count: bool,
    /// Add up this numeric column over the matching rows, per group when `group_by` is set.
    #[serde(default)]
    sum: Option<String>,
}

pub struct CsvQuery {
    workspace: Workspace,
    spec: ToolSpec,
}

impl CsvQuery {
    pub fn new(workspace: Workspace) -> Self {
        let purpose = "Filter, select, count, sum or group the rows of a CSV file in the workspace. No expressions.";
        Self {
            workspace,
            spec: spec::<CsvArgs>(ToolName::CsvQuery, purpose, TIMEOUT),
        }
    }
}

#[async_trait]
impl Tool for CsvQuery {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn check(&self, args: &Value) -> Result<Action, ToolError> {
        let args: CsvArgs = parse_args(args)?;
        if !args.path.to_ascii_lowercase().ends_with(".csv") {
            return Err(ToolError::InvalidArgs(
                "path: must be a .csv file".to_string(),
            ));
        }
        file_action("query", &args.path)
    }

    async fn reach(&self, team: TeamId, args: &Value) -> Result<(), ToolError> {
        let args: CsvArgs = parse_args(args)?;
        self.workspace.reach(team, &args.path, Need::Read).await
    }

    async fn run(&self, ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: CsvArgs = parse_args(&args)?;
        let at = self
            .workspace
            .locate(ctx.team_id, &args.path, Need::Read)
            .await?;
        let (text, trust) = self.workspace.read_with_trust(&at.physical).await?;
        let path = at.shown;
        Ok(ToolOutput {
            content: query(&text, &args, &path)?,
            taint: file_taint(&path, trust),
        })
    }
}

/// Filters the rows, then sums, counts or groups them, or projects the selected columns.
fn query(text: &str, args: &CsvArgs, path: &str) -> Result<Value, ToolError> {
    let not_csv = || ToolError::Failed(format!("{path} is not valid CSV"));
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    let headers: Vec<String> = reader
        .headers()
        .map_err(|_| not_csv())?
        .iter()
        .map(str::to_string)
        .collect();
    let column = |name: &str| {
        headers
            .iter()
            .position(|h| h == name)
            .ok_or_else(|| ToolError::InvalidArgs(format!("unknown column: {name}")))
    };

    let filters = args
        .filter
        .iter()
        .map(|f| Ok((column(&f.column)?, f.op, f.value.as_str())))
        .collect::<Result<Vec<_>, ToolError>>()?;
    let mut matching = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|_| not_csv())?;
        let keep = filters
            .iter()
            .all(|(index, op, value)| matches(record.get(*index).unwrap_or(""), *op, value));
        if keep {
            matching.push(record);
        }
    }

    // Models often send empty defaults for fields they do not use; empty means absent.
    let group_by = args.group_by.as_deref().filter(|group| !group.is_empty());
    let select: Vec<&str> = args
        .select
        .iter()
        .map(String::as_str)
        .filter(|name| !name.is_empty())
        .collect();
    let sum = args.sum.as_deref().filter(|name| !name.is_empty());
    let result = if let Some(name) = sum {
        let index = column(name)?;
        let mut totals: BTreeMap<String, Decimal> = BTreeMap::new();
        for (row, record) in matching.iter().enumerate() {
            let cell = record.get(index).unwrap_or("");
            let value = Decimal::parse(cell).ok_or_else(|| {
                ToolError::Failed(format!(
                    "{path}: {name} is not a number in matching row {}: {cell:?}",
                    row + 1
                ))
            })?;
            let key = match group_by {
                Some(group) => record.get(column(group)?).unwrap_or("").to_string(),
                None => String::new(),
            };
            let total = totals.entry(key).or_default();
            *total = total.add(value).ok_or_else(|| {
                ToolError::Failed(format!("{path}: the sum of {name} is too large"))
            })?;
        }
        match group_by {
            Some(group) => {
                let sums: BTreeMap<String, String> = totals
                    .into_iter()
                    .map(|(k, v)| (k, v.to_string()))
                    .collect();
                json!({ "sum_of": name, "group_by": group, "sums": sums, "rows": matching.len() })
            }
            None => {
                let total = totals.remove("").unwrap_or_default();
                json!({ "sum_of": name, "sum": total.to_string(), "rows": matching.len() })
            }
        }
    } else if let Some(group) = group_by {
        let index = column(group)?;
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for record in &matching {
            *counts
                .entry(record.get(index).unwrap_or("").to_string())
                .or_insert(0) += 1;
        }
        json!({ "group_by": group, "counts": counts })
    } else if args.count {
        json!({ "count": matching.len() })
    } else {
        let selected = if select.is_empty() {
            (0..headers.len()).collect()
        } else {
            select
                .iter()
                .map(|name| column(name))
                .collect::<Result<Vec<_>, ToolError>>()?
        };
        let rows: Vec<Vec<&str>> = matching
            .iter()
            .take(ROW_LIMIT)
            .map(|record| {
                selected
                    .iter()
                    .map(|i| record.get(*i).unwrap_or(""))
                    .collect()
            })
            .collect();
        let columns: Vec<&str> = selected.iter().map(|i| headers[*i].as_str()).collect();
        json!({ "columns": columns, "rows": rows, "total": matching.len() })
    };
    Ok(result)
}

fn matches(cell: &str, op: FilterOp, value: &str) -> bool {
    let numbers = cell
        .trim()
        .parse::<f64>()
        .ok()
        .zip(value.trim().parse::<f64>().ok());
    match op {
        FilterOp::Eq => cell == value,
        FilterOp::Ne => cell != value,
        FilterOp::Contains => cell.to_lowercase().contains(&value.to_lowercase()),
        FilterOp::Gt => numbers.map_or(cell > value, |(c, v)| c > v),
        FilterOp::Lt => numbers.map_or(cell < value, |(c, v)| c < v),
    }
}

/// An exact decimal for sums: millionths in an `i128`, so money adds up without float error.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Decimal {
    millionths: i128,
    /// The most fraction digits any summed value had, so `17899.50` prints as written.
    places: u32,
}

impl Decimal {
    const PLACES: u32 = 6;

    /// `-12`, `12.5`, `1200.00`; at most six fraction digits, no thousands separators.
    fn parse(cell: &str) -> Option<Decimal> {
        let cell = cell.trim();
        let (negative, digits) = match cell.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, cell),
        };
        let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
        let all_digits = |part: &str| part.chars().all(|c| c.is_ascii_digit());
        if whole.is_empty() || !all_digits(whole) || !all_digits(fraction) {
            return None;
        }
        let places = u32::try_from(fraction.len())
            .ok()
            .filter(|p| *p <= Self::PLACES)?;
        if digits.ends_with('.') {
            return None;
        }
        let whole: i128 = whole.parse().ok()?;
        let fraction: i128 = if fraction.is_empty() {
            0
        } else {
            fraction.parse().ok()?
        };
        let scale = 10_i128.pow(Self::PLACES);
        let millionths = whole
            .checked_mul(scale)?
            .checked_add(fraction * 10_i128.pow(Self::PLACES - places))?;
        Some(Decimal {
            millionths: if negative { -millionths } else { millionths },
            places,
        })
    }

    fn add(self, other: Decimal) -> Option<Decimal> {
        Some(Decimal {
            millionths: self.millionths.checked_add(other.millionths)?,
            places: self.places.max(other.places),
        })
    }
}

impl std::fmt::Display for Decimal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scale = 10_i128.pow(Self::PLACES);
        let sign = if self.millionths < 0 { "-" } else { "" };
        let abs = self.millionths.abs();
        let (whole, fraction) = (abs / scale, abs % scale);
        if self.places == 0 {
            return write!(f, "{sign}{whole}");
        }
        let fraction = fraction / 10_i128.pow(Self::PLACES - self.places);
        write!(
            f,
            "{sign}{whole}.{fraction:0width$}",
            width = self.places as usize
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(cells: &[&str]) -> Option<String> {
        cells
            .iter()
            .try_fold(Decimal::default(), |total, cell| {
                total.add(Decimal::parse(cell)?)
            })
            .map(|total| total.to_string())
    }

    #[test]
    fn sums_are_exact_and_keep_the_written_places() {
        assert_eq!(sum(&["1200.00", "16699.00"]).as_deref(), Some("17899.00"));
        assert_eq!(sum(&["0.1", "0.2"]).as_deref(), Some("0.3"));
        assert_eq!(sum(&["-5", "2.50"]).as_deref(), Some("-2.50"));
        assert_eq!(sum(&["7"]).as_deref(), Some("7"));
        for bad in ["", "1,200", "12.", ".5", "1e3", "abc", "1.1234567"] {
            assert_eq!(Decimal::parse(bad), None, "{bad:?}");
        }
    }
}
