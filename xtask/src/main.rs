//! Development tasks for tokendraw. Run as `cargo xtask <task>`.

mod ember;
mod open_weight_sets;
mod price_calibration;
#[cfg(test)]
mod release_pins;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use jiff::civil::Date;

#[derive(Parser)]
#[command(about = "Development tasks for tokendraw")]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Write data/grid/ember.toml from Ember's yearly global and US state CSV files
    Ember {
        /// Ember's yearly electricity data for countries and regions
        global: PathBuf,
        /// Ember's yearly US electricity data by state
        us: PathBuf,
        /// The URL the global file was downloaded from
        #[arg(long)]
        global_url: String,
        /// The URL the US file was downloaded from
        #[arg(long)]
        us_url: String,
        /// The date both files were downloaded, YYYY-MM-DD; also the review date of every value
        #[arg(long)]
        accessed: Date,
    },
    /// Write data/energy/price-calibration.toml from the endpoint files of the models in xtask/price-calibration.toml
    PriceCalibration {
        /// One file per slug, from `https://openrouter.ai/api/v1/models/<slug>/endpoints`
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// The date the files were downloaded, YYYY-MM-DD; also the review date of the calibration
        #[arg(long)]
        accessed: Date,
    },
    /// Write data/energy/open-weight-sets.toml from the attention layouts in xtask/open-weight-sets.toml
    OpenWeightSets {
        /// The date the inputs were last reviewed, YYYY-MM-DD; the review date of every set
        #[arg(long)]
        reviewed: Date,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().task {
        Task::Ember {
            global,
            us,
            global_url,
            us_url,
            accessed,
        } => run_ember(&global, &us, global_url, us_url, accessed),
        Task::PriceCalibration { files, accessed } => run_price_calibration(&files, accessed),
        Task::OpenWeightSets { reviewed } => run_open_weight_sets(reviewed),
    }
}

fn run_open_weight_sets(reviewed: Date) -> anyhow::Result<()> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
    let read = |path: &str| {
        let path = workspace.join(path);
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))
    };
    let input = open_weight_sets::parse_input(&read("open-weight-sets.toml")?)?;
    let weights = price_calibration::parse_input(&read("price-calibration.toml")?)?;
    let parameter_sets = read("../data/energy/parameter-sets.toml")?;
    let sets = open_weight_sets::compute(&input, &weights)?;
    let text = open_weight_sets::render(&input, &weights, &sets, &parameter_sets, reviewed)?;
    let out = workspace.join("../data/energy/open-weight-sets.toml");
    fs::write(&out, text).with_context(|| format!("cannot write {}", out.display()))?;
    eprintln!("wrote {} parameter sets to {}", sets.len(), out.display());
    for set in &sets {
        let v = &set.central;
        eprintln!(
            "  {}: central a_p {:.4} uJ, a_d {:.4} uJ, b_in {:.4} J, b_out {:.4} J, c0 {:.4} mJ",
            set.id, v.a_p, v.a_d, v.b_in, v.b_out, v.c0
        );
    }
    Ok(())
}

fn run_price_calibration(paths: &[PathBuf], accessed: Date) -> anyhow::Result<()> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
    let input_path = workspace.join("price-calibration.toml");
    let input_text = fs::read_to_string(&input_path)
        .with_context(|| format!("cannot read {}", input_path.display()))?;
    let input = price_calibration::parse_input(&input_text)?;
    let files = paths
        .iter()
        .map(|path| {
            let bytes = fs::read(path)
                .with_context(|| format!("cannot read {}; check the path", path.display()))?;
            Ok(price_calibration::EndpointFile {
                name: path.display().to_string(),
                sha256: ember::sha256_hex(&bytes),
                bytes,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let calibration = price_calibration::calibrate(&input, &files)?;
    let text = price_calibration::render(&input, &calibration, &files, accessed)?;
    let out = workspace.join("../data/energy/price-calibration.toml");
    fs::write(&out, text).with_context(|| format!("cannot write {}", out.display()))?;
    let [
        envelope_low,
        plausible_low,
        central,
        plausible_high,
        envelope_high,
    ] = calibration.five;
    eprintln!(
        "wrote {}: {} host and model cells, {} endpoints left out; Wh per USD {envelope_low:.4} / {plausible_low:.4} / {central:.4} / {plausible_high:.4} / {envelope_high:.4}",
        out.display(),
        calibration.cells,
        calibration.skipped,
    );
    Ok(())
}

fn run_ember(
    global: &Path,
    us: &Path,
    global_url: String,
    us_url: String,
    accessed: Date,
) -> anyhow::Result<()> {
    let read = |path: &Path| {
        fs::read(path).with_context(|| format!("cannot read {}; check the path", path.display()))
    };
    let global_bytes = read(global)?;
    let us_bytes = read(us)?;
    let extract = ember::extract(&global_bytes, &us_bytes)?;
    let text = ember::render(
        &extract,
        &ember::Meta {
            global_url,
            global_sha256: ember::sha256_hex(&global_bytes),
            us_url,
            us_sha256: ember::sha256_hex(&us_bytes),
            accessed,
        },
    );
    // The output belongs to the workspace, wherever the command is run from.
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/grid/ember.toml");
    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    fs::write(&out, text).with_context(|| format!("cannot write {}", out.display()))?;
    eprintln!(
        "wrote {} geographies to {}; years rejected for a negative total: {}",
        extract.geographies.len(),
        out.display(),
        extract.rejected.len()
    );
    Ok(())
}
