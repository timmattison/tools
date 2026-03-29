use anyhow::Result;
use clap::ValueEnum;
use serde::Serialize;
use tabled::{Table, Tabled, settings::Style};

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum OutputFormat {
    Json,
    Table,
}

pub fn print_output<T>(data: &T, format: OutputFormat) -> Result<()>
where
    T: Serialize + ?Sized,
{
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(data)?);
        }
        OutputFormat::Table => {
            println!("{}", serde_json::to_string_pretty(data)?);
        }
    }
    Ok(())
}

pub fn print_table<T>(data: &[T]) -> Result<()>
where
    T: Tabled,
{
    let mut table = Table::new(data);
    table.with(Style::modern());
    println!("{}", table);
    Ok(())
}


// Specific implementation for vectors with Tabled items
pub fn print_vec_table<T>(data: &[T], format: OutputFormat) -> Result<()>
where
    T: Serialize + Tabled,
{
    match format {
        OutputFormat::Json => print_output(data, format),
        OutputFormat::Table => print_table(data),
    }
}

// General implementation for any serializable type
pub fn print_single_item<T>(data: &T, format: OutputFormat) -> Result<()>
where
    T: Serialize,
{
    print_output(data, format)
}