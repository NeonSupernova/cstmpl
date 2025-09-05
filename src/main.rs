use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use chrono::Local;
use clap::{Parser, Subcommand, Args};
use inquire::{Text, Select};
use tera::{Tera, Context};
use directories::BaseDirs;
use serde_yaml::Value;

#[derive(Parser)]
#[command(name = "cstmpl")]
#[command(about = "Custom template generator", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a file from a template
    Gen(GenArgs),
    /// Initialize a new template interactively
    Init,
    /// Edit a template in the default text editor
    Edit {
        /// Template name
        template: String,
    },
    /// List all available templates
    List,
    /// Remove a template
    Remove {
        /// Template name
        template: String,
    },
}

#[derive(Args)]
struct GenArgs {
    /// Template name
    template: String,
    /// Output file
    output: PathBuf,
    /// Preview only (render to stdout)
    #[arg(long)]
    preview: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Gen(args) => {
            let template_path = find_template_path(&args.template)?;
            let template_str = fs::read_to_string(&template_path)?;

            // Parse YAML front matter if exists
            let (variables, template_body) = parse_front_matter(&template_str)?;

            let mut context = Context::new();
            for (var, info) in &variables {
                let var_name = var.as_str().unwrap();
                let default_val = info.get("default")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let var_type = info.get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("string");

                let val = if default_val == "today" {
                    Local::now().format("%Y-%m-%d").to_string()
                } else {
                    prompt_for_var(var_name, var_type, Some(&default_val))?
                };
                context.insert(var_name, &val);
            }

            let mut tera = Tera::default();
            tera.register_function("header", make_header);
            tera.add_raw_template("tpl", &template_body)?;
            let rendered = tera.render("tpl", &context)?;

            if args.preview {
                println!("{}", rendered);
            } else {
                fs::write(&args.output, rendered)?;
            }
        }
        Commands::Init => {
            interactive_init()?;
        }
        Commands::Edit { template } => {
            let template_path = find_template_path(&template)?;
            let editor = std::env::var("EDITOR")
                .unwrap_or_else(|_| {
                    if cfg!(windows) { "notepad".to_string() } else { "nano".to_string() }
                });
            let status = Command::new(editor)
                .arg(&template_path)
                .status()?;

            if !status.success() {
                eprintln!("Editor exited with a non-zero status");
            }
        }
        Commands::List => {
            let templates = collect_templates()?;
            if templates.is_empty() {
                println!("No templates found.");
            } else {
                for t in templates {
                    println!("{}", t);
                }
            }
        }
        Commands::Remove { template } => {
            let template_path = find_template_path(&template)?;
            let confirm = inquire::Confirm::new(&format!("Are you sure you want to delete template '{}'?",&template))
                .with_default(false)
                .prompt()?;
            if confirm {
                fs::remove_file(&template_path)?;
                println!("Template '{}' has been deleted.", template);
            } else {
                println!("Deletion cancelled.");
            }
        }
    }

    Ok(())
}

fn prompt_for_var(name: &str, var_type: &str, default: Option<&str>) -> anyhow::Result<String> {
    let prompt_str = format!("Enter value for {} (type: {}):", name, var_type);
    let mut prompt = Text::new(&prompt_str);
    if let Some(d) = default {
        prompt = prompt.with_initial_value(d);
    }
    let val = prompt.prompt().unwrap_or_default();
    Ok(val)
}

fn interactive_init() -> anyhow::Result<()> {
    // Ask for template name
    let name: String = Text::new("Template name:").prompt()?;
    let path = xdg_template_dir()?.join(format!("{name}.tpl"));
    if path.exists() {
        anyhow::bail!("Template {} already exists", name);
    }

    fs::create_dir_all(path.parent().unwrap())?;

    // Ask for variables
    let mut vars: serde_yaml::Mapping = serde_yaml::Mapping::new();
    loop {
        let var_name: String = Text::new("Variable name (empty to finish):").prompt()?;
        if var_name.is_empty() {
            break;
        }
        let var_type_choices = vec!["string", "number", "boolean"];
        let var_type = Select::new("Type:", var_type_choices).prompt()?;
        let default_val: String = Text::new("Default value (empty for none, use 'today' for date):").prompt()?;
        let mut info = serde_yaml::Mapping::new();
        info.insert(Value::String("type".to_string()), Value::String(var_type.to_string()));
        info.insert(Value::String("default".to_string()), Value::String(default_val));
        vars.insert(Value::String(var_name), Value::Mapping(info));
    }

    // Create template content
    let mut content = String::new();
    if !vars.is_empty() {
        content.push_str("---\n");
        content.push_str(&serde_yaml::to_string(&vars)?);
        content.push_str("---\n\n");
    }

    content.push_str(&format!("/*\n * Template: {name}\n */\n\n#include <stdio.h>\n\nint main(void) {{\n    printf(\"Hello {{ name }}!\\n\");\n    return 0;\n}}\n"));

    fs::write(&path, content)?;
    println!("Created template at {}", path.display());
    Ok(())
}

fn collect_templates() -> anyhow::Result<Vec<String>> {
    let mut templates = vec![];
    let local_dir = Path::new("templates");
    if local_dir.exists() {
        for entry in fs::read_dir(local_dir)? {
            let e = entry?;
            if e.path().extension().map(|s| s == "tpl").unwrap_or(false) {
                if let Some(name) = e.path().file_stem().and_then(|s| s.to_str()) {
                    templates.push(name.to_string());
                }
            }
        }
    }

    let xdg_dir = xdg_template_dir()?;
    if xdg_dir.exists() {
        for entry in fs::read_dir(xdg_dir)? {
            let e = entry?;
            if e.path().extension().map(|s| s == "tpl").unwrap_or(false) {
                if let Some(name) = e.path().file_stem().and_then(|s| s.to_str()) {
                    if !templates.contains(&name.to_string()) {
                        templates.push(name.to_string());
                    }
                }
            }
        }
    }
    Ok(templates)
}

fn parse_front_matter(template: &str) -> anyhow::Result<(serde_yaml::Mapping, String)> {
    if template.starts_with("---") {
        if let Some(end) = template[3..].find("---") {
            let yaml_str = &template[3..3 + end];
            let rest = &template[3 + end + 3..];
            let vars: serde_yaml::Mapping = serde_yaml::from_str(yaml_str)?;
            return Ok((vars, rest.to_string()));
        }
    }
    Ok((serde_yaml::Mapping::new(), template.to_string()))
}

fn find_template_path(name: &str) -> anyhow::Result<PathBuf> {
    let local = PathBuf::from(format!("templates/{name}.tpl"));
    if local.exists() {
        return Ok(local);
    }

    let xdg = xdg_template_dir()?.join(format!("{name}.tpl"));
    if xdg.exists() {
        return Ok(xdg);
    }

    anyhow::bail!("Template '{name}' not found in local or XDG directories.");
}

fn xdg_template_dir() -> anyhow::Result<PathBuf> {
    if let Some(base) = BaseDirs::new() {
        Ok(base.data_dir().join("cstmpl/templates"))
    } else {
        anyhow::bail!("Could not determine XDG data dir")
    }
}

fn make_header(message: &str) -> String {
    const WIDTH: usize = 72;
    let border = "/".to_string() + &"*".repeat(WIDTH - 2) + "/";

    // Wrap text into lines no longer than WIDTH - 4 (for "/* " and " */")
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in message.split_whitespace() {
        if current.len() + word.len() + 1 > WIDTH - 4 {
            lines.push(current.trim_end().to_string());
            current.clear();
        }
        current.push_str(word);
        current.push(' ');
    }
    if !current.is_empty() {
        lines.push(current.trim_end().to_string());
    }

    // Format into header block
    let mut result = String::new();
    result.push_str(&border);
    result.push('\n');
    for line in lines {
        result.push_str("/* ");
        result.push_str(&format!("{:<width$}", line, width = WIDTH - 6));
        result.push_str(" */\n");
    }
    result.push_str(&border);

    result
}