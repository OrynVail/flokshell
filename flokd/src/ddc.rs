use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};

const I2C_SLAVE: libc::c_ulong = 0x0703;
const EDID: u16 = 0x50;
const DDC: u16 = 0x37;
const HOST: u8 = 0x51;
const DISPLAY: u8 = 0x6E;

pub const BRIGHTNESS: u8 = 0x10;

// DDC/CI 1.1: a display may take 40ms to answer and 50ms to accept the next
// command. Cheap monitors drop requests outright, hence the retries.
const REPLY_DELAY: Duration = Duration::from_millis(40);
pub const SETTLE: Duration = Duration::from_millis(50);
const ATTEMPTS: u32 = 3;

fn open(bus: u32, addr: u16) -> Result<File> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(format!("/dev/i2c-{bus}"))?;
    // SAFETY: I2C_SLAVE takes the address by value; the fd is open.
    if unsafe { libc::ioctl(f.as_raw_fd(), I2C_SLAVE, addr as libc::c_ulong) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(f)
}

// One write() and one read() each are one i2c transaction; read_exact could split one.
fn transact(f: &mut File, out: &[u8], wait: Duration, back: &mut [u8]) -> Result<()> {
    if f.write(out)? != out.len() {
        bail!("short write");
    }
    sleep(wait);
    if f.read(back)? != back.len() {
        bail!("short read");
    }
    Ok(())
}

pub fn edid(bus: u32) -> Result<[u8; 128]> {
    let mut f = open(bus, EDID)?;
    let mut block = [0u8; 128];
    transact(&mut f, &[0], Duration::ZERO, &mut block)?;
    Ok(block)
}

fn checksum(init: u8, bytes: &[u8]) -> u8 {
    bytes.iter().fold(init, |c, b| c ^ b)
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut msg = vec![HOST, 0x80 | payload.len() as u8];
    msg.extend_from_slice(payload);
    msg.push(checksum(DISPLAY, &msg));
    msg
}

/// (current, max)
fn parse_vcp(code: u8, r: &[u8; 11]) -> Result<(u16, u16)> {
    if r[1] == 0x80 {
        bail!("display busy");
    }
    // Checksummed as if from the host's 0x50, not the 0x6E the reply carries.
    if checksum(0x50, &r[..10]) != r[10] {
        bail!("bad checksum");
    }
    if r[0] != DISPLAY || r[1] != 0x88 || r[2] != 0x02 || r[4] != code {
        bail!("unexpected reply {r:02x?}");
    }
    if r[3] != 0 {
        bail!("vcp {code:#04x} unsupported");
    }
    Ok((
        u16::from_be_bytes([r[8], r[9]]),
        u16::from_be_bytes([r[6], r[7]]),
    ))
}

pub fn get_vcp(bus: u32, code: u8) -> Result<(u16, u16)> {
    let mut f = open(bus, DDC)?;
    let req = frame(&[0x01, code]);
    let mut last = anyhow!("no attempt");
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            sleep(SETTLE);
        }
        let mut r = [0u8; 11];
        match transact(&mut f, &req, REPLY_DELAY, &mut r).and_then(|()| parse_vcp(code, &r)) {
            Ok(v) => return Ok(v),
            Err(e) => last = e,
        }
    }
    Err(last)
}

pub fn set_vcp(bus: u32, code: u8, value: u16) -> Result<()> {
    let mut f = open(bus, DDC)?;
    let [hi, lo] = value.to_be_bytes();
    let req = frame(&[0x03, code, hi, lo]);
    let mut last = anyhow!("no attempt");
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            sleep(SETTLE);
        }
        match f.write(&req) {
            Ok(n) if n == req.len() => return Ok(()),
            Ok(_) => last = anyhow!("short write"),
            Err(e) => last = e.into(),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_carry_length_and_checksum() {
        assert_eq!(frame(&[0x01, 0x10]), [0x51, 0x82, 0x01, 0x10, 0xac]);
        assert_eq!(
            frame(&[0x03, 0x10, 0x00, 0x3c]),
            [0x51, 0x84, 0x03, 0x10, 0x00, 0x3c, 0x94]
        );
    }

    // Captured from a Gigabyte G24F 2 on an NVIDIA i2c adapter.
    const G24F: [u8; 11] = [
        0x6e, 0x88, 0x02, 0x00, 0x10, 0x00, 0x00, 0x64, 0x00, 0x41, 0x81,
    ];

    #[test]
    fn reply_reads_current_and_max() {
        assert_eq!(parse_vcp(0x10, &G24F).unwrap(), (65, 100));
    }

    #[test]
    fn reply_rejects_corruption_and_refusal() {
        let mut bad = G24F;
        bad[9] = 0x42;
        assert!(parse_vcp(0x10, &bad).is_err());
        assert!(parse_vcp(0x12, &G24F).is_err());

        let mut refused = G24F;
        refused[3] = 0x01;
        refused[10] = checksum(0x50, &refused[..10]);
        assert!(parse_vcp(0x10, &refused).is_err());
    }
}
