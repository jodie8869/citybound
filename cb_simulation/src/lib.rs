#![recursion_limit = "128"]
extern crate ordered_float;
extern crate itertools;
extern crate rand;
extern crate noise;
extern crate fnv;
extern crate roaring;
extern crate uuid;

pub extern crate compact;
#[macro_use]
extern crate compact_macros;
pub extern crate kay;
pub extern crate michelangelo;
pub extern crate descartes;

#[macro_use]
extern crate serde_derive;

pub mod util;
pub mod log;
pub mod time;
pub mod transport;
pub mod planning;
pub mod construction;
pub mod economy;
pub mod land_use;
pub mod dimensions;
pub mod environment;

pub fn setup_common(system: &mut kay::ActorSystem) {
    for setup_fn in &[
        time::setup,
        log::setup,
        planning::setup,
        construction::setup,
        transport::setup,
        economy::setup,
        land_use::setup,
        environment::setup,
    ] {
        setup_fn(system)
    }
}
