fn main() {
    olo_crash::install();
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 4 {
        eprintln!("usage: olo-web-eval SUITE.json CHROME_RUN OLO_RUN REPORT.json");
        std::process::exit(2);
    }
    match olo_web_eval::evaluate(std::path::Path::new(&args[0]), std::path::Path::new(&args[1]), std::path::Path::new(&args[2])) {
        Ok(report) => {
            if let Err(e) = std::fs::write(&args[3], serde_json::to_vec_pretty(&report).unwrap()) {
                eprintln!("cannot write report: {e}");
                std::process::exit(2);
            }
            println!(
                "{} of {} cases passed; {} failed, {} blocked, {} missing. {} of {} sites passed all configurations.",
                report.passed, report.total, report.failed, report.blocked, report.missing, report.sites_passed, report.sites_total
            );
            if report.passed != report.total {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("invalid evaluation: {e}");
            std::process::exit(2);
        }
    }
}
