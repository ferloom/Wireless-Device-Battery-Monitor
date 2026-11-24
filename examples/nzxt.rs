//! Read only the Lift Elite telemetry channel: cargo run --example nzxt
use batteryhub::{device::PollContext, drivers::nzxt, hid};

fn main() {
    let colls =
        hid::enum_hid().into_iter().filter(|c| c.vid == 0x1E71 && matches!(c.pid, 0x2101 | 0x2129) && c.page() == 0xFFCA && c.usage() == 1).collect::<Vec<_>>();
    println!("Found {} Lift Elite telemetry interface(s).", colls.len());
    let mut ctx = PollContext { colls, ..Default::default() };
    nzxt::poll(&mut ctx);
    if ctx.out.is_empty() {
        println!("No telemetry received. Close NZXT CAM and wake the mouse, then retry.");
    }
    for d in ctx.out {
        println!("{}: {} via {} ({} mV)", d.name, d.status_text(), d.via, d.millivolts);
    }
}
