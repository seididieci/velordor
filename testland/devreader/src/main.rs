//! userdevreader — riproduttore: legge ripetutamente 4096 byte da `/dev/zero`
//! (device remoto via vela) mentre `userhogheap` genera page-fault demand-zero.
//! Se il lost-wakeup si manifesta, il reader si blocca in attesa di una reply
//! che non arriva e smette di stampare progressi.

#![no_std]
#![no_main]

use civis;

civis::entry!(real_main);
fn real_main(_sp: u64) -> ! {
    let _ = civis::print_string(b"[devreader] starting\n");

    // Retry finche' vela non ha registrato /dev (race di boot).
    let fd = loop {
        if let Ok(fd) = civis::open("/dev/zero", 0) {
            break fd;
        }
        for _ in 0..1_000_000 {
            core::hint::spin_loop();
        }
    };

    let mut buf = [0xFFu8; 4096];
    let mut round: u32 = 0;
    loop {
        let n = civis::read_fs(fd, &mut buf, 4096).unwrap_or(0);
        if n != 4096 {
            let _ = civis::print_string(b"[devreader] short read / FAIL\n");
            civis::exit(1);
        }
        // Tutto zero?
        let mut all_zero = true;
        for i in 0..4096 {
            if buf[i] != 0 {
                all_zero = false;
                break;
            }
        }
        if !all_zero {
            let _ = civis::print_string(b"[devreader] non-zero data\n");
            civis::exit(1);
        }

        round += 1;
        if round % 25 == 0 {
            let _ = civis::print_string(b"[devreader] round\n");
        }
        if round >= 400 {
            let _ = civis::print_string(b"[devreader] DONE\n");
            civis::exit(0);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    let _ = civis::print_string(b"[devreader] panic\n");
    civis::exit(1)
}
