// Throwaway sanity check for the spec_ingest pipeline:
//   cargo run -p sim-flow --release --bin probe_ingest -- <spec> <project>

use sim_flow::session::spec_ingest::ingest_spec_file;

fn main() {
    let mut args = std::env::args().skip(1);
    let spec = args.next().expect("usage: probe_ingest <spec> <project>");
    let project = args.next().expect("usage: probe_ingest <spec> <project>");
    let project = std::path::PathBuf::from(project);
    std::fs::create_dir_all(project.join(".sim-flow")).unwrap();
    match ingest_spec_file(std::path::Path::new(&spec), &project) {
        Ok(summary) => {
            println!(
                "ok: {} pages, toc={}",
                summary.page_count,
                summary.toc_path.display()
            );
        }
        Err(err) => {
            eprintln!("err: {err}");
            std::process::exit(1);
        }
    }
}
