//! `pixie-check`: reads a skill (JSON on stdin) and says whether Pixie would
//! accept it. Prints `ok`, or the reason, and exits non-zero on a refusal.
//! The bridge on the computer runs this before sending a skill to the phone.

use pixie_script::SkillSpec;
use std::io::Read;

fn main() {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        eprintln!("could not read input");
        std::process::exit(2);
    }
    match serde_json::from_str::<SkillSpec>(&input) {
        Err(e) => {
            println!("not a skill: {e}");
            std::process::exit(1);
        }
        Ok(spec) => match spec.validate() {
            Ok(()) => println!("ok"),
            Err(e) => {
                println!("{e}");
                std::process::exit(1);
            }
        },
    }
}
