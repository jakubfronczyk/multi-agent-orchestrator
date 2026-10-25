//! `csv_query`: filter, select, count or group the rows of a workspace CSV file.
//! It reads through `Workspace`, so trust and taint follow the same rule as `file_read`.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use orchestrator_core::tools::{
    Action, Tool, ToolCtx, ToolError, ToolName, ToolOutput, ToolSpec, parse_args,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::files::{Workspace, display, file_action, file_taint};
use super::sandbox::relative;
use super::spec;

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
    /// Columns to return; all columns when absent.
    #[serde(default)]
    select: Vec<String>,
    /// Count matching rows per value of this column.
    #[serde(default)]
    group_by: Option<String>,
    /// Return only the number of matching rows.
    #[serde(default)]
    count: bool,
}

pub struct CsvQuery {
    workspace: Workspace,
    spec: ToolSpec,
}

impl CsvQuery {
    pub fn new(workspace: Workspace) -> Self {
        let purpose = "Filter, select, count or group the rows of a CSV file in the workspace. No expressions.";
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

    async fn run(&self, _ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: CsvArgs = parse_args(&args)?;
        let path = display(&relative(&args.path)?);
        let (text, trust) = self.workspace.read_with_trust(&path).await?;
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

        let result = if let Some(group) = &args.group_by {
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
            let selected = if args.select.is_empty() {
                (0..headers.len()).collect()
            } else {
                args.select
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

        Ok(ToolOutput {
            content: result,
            taint: file_taint(&path, trust.as_deref()),
        })
    }
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
