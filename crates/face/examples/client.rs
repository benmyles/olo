//! Drives face unlock as Olo's apps do (`OLO_FACE_BUS`, a private bus):
//! `client status | enroll PASSWORD | verify | forget PASSWORD | pam PASSWORD`
//! (`pam`: PAM's check through `OLO_PAM_CONFDIR`/`OLO_PAM_SERVICE`, a
//! debug build's stand-in, with the face module in it).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let t = std::time::Instant::now();
    match args.first().map(String::as_str) {
        Some("status") => println!("{:?}", olo_system::face::status()),
        Some("enroll") => println!("{:?}", olo_system::face::enroll(&args[1], |got, of| println!("  {got} of {of}"))),
        Some("verify") => println!("{:?}", olo_system::face::verify(4000)),
        Some("forget") => println!("{:?}", olo_system::face::forget(&args[1])),
        Some("pam") => {
            let user = olo_system::auth::current_user().unwrap_or_default();
            println!("{:?}", olo_system::auth::check_password(&user, &args[1]));
        }
        _ => eprintln!("client status | enroll PASSWORD | verify | forget PASSWORD | pam PASSWORD"),
    }
    println!("  ({:.1} s)", t.elapsed().as_secs_f64());
}
