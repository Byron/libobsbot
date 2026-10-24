// SPDX-License-Identifier: GPL-3.0-only
//! Tiny 3 controls from public protocol notes and the connected full Tiny 3.
//! Sources and hardware observations: `doc/protocol/tiny3/README.md`.

use super::meet2::crc16_usb;
use crate::{Error, Result};

pub(crate) const XU_ENTITY_ID: u8 = 2;
pub(crate) const XU_SEL_STATUS: u8 = 6;
pub(crate) const XU_SEL_RPC: u8 = 2;
pub(crate) const FRAME_LEN: usize = 60;
const SERIAL_COMMAND: u16 = 0x18c8;

pub(crate) fn tracking_payload(enabled: bool) -> [u8; FRAME_LEN] {
    let mut frame = [0; FRAME_LEN];
    frame[..4].copy_from_slice(&[0x16, 2, if enabled { 2 } else { 0 }, 0]);
    frame
}

pub(crate) fn decode_ai(status: &[u8]) -> Result<(u8, u8)> {
    if status.len() == FRAME_LEN && status[0] == 0x2e {
        let mode = (status[0x18], status[0x1c]);
        if matches!(mode, (0..=5, 0) | (2, 1..=4)) {
            return Ok(mode);
        }
    }
    Err(Error::BadResponse {
        selector: XU_SEL_STATUS,
        bytes: status.to_vec(),
    })
}

pub(crate) fn decode_tracking(status: &[u8]) -> Result<bool> {
    Ok(decode_ai(status)?.0 != 0)
}

pub(crate) fn asleep(status: &[u8]) -> Result<bool> {
    if status.len() == FRAME_LEN && status[0] == 0x2e && status[2] <= 1 {
        return Ok(status[2] != 0);
    }
    Err(Error::BadResponse {
        selector: XU_SEL_STATUS,
        bytes: status.to_vec(),
    })
}

pub(crate) fn build_serial_request(seq: u16) -> [u8; FRAME_LEN] {
    build_header(0x01, seq, 0x0d, SERIAL_COMMAND)
}

pub(crate) fn build_power_request(seq: u16, standby: bool) -> [u8; FRAME_LEN] {
    let mut frame = build_header(0x25, seq, 0x02, 0xa0c2);
    frame[12] = 4;
    frame[16] = u8::from(standby);
    let crc = crc16_usb(&frame[12..20]);
    frame[14..16].copy_from_slice(&crc.to_le_bytes());
    frame
}

fn build_header(flags: u8, seq: u16, receiver: u8, command: u16) -> [u8; FRAME_LEN] {
    let mut frame = [0; FRAME_LEN];
    frame[0] = 0xaa;
    frame[1] = flags;
    frame[2..4].copy_from_slice(&seq.to_le_bytes());
    frame[4] = 12;
    frame[8] = 0x0a;
    frame[9] = receiver;
    frame[10..12].copy_from_slice(&command.to_le_bytes());
    let crc = crc16_usb(&frame[..12]);
    frame[6..8].copy_from_slice(&crc.to_le_bytes());
    frame
}

pub(crate) fn parse_serial_reply(raw: &[u8], seq: u16) -> Option<&[u8]> {
    if raw.len() != FRAME_LEN
        || raw[..2] != [0xaa, 0x29]
        || raw[2..4] != seq.to_le_bytes()
        || raw[4..6] != [12, 0]
        || raw[8..10] != [0x0d, 0x0a]
        || raw[10..12] != SERIAL_COMMAND.to_le_bytes()
    {
        return None;
    }
    let payload_len = usize::from(u16::from_le_bytes([raw[12], raw[13]]));
    if !(1..=FRAME_LEN - 16).contains(&payload_len) {
        return None;
    }
    let mut checked = [0; FRAME_LEN];
    checked.copy_from_slice(raw);
    checked[6..8].fill(0);
    checked[14..16].fill(0);
    if crc16_usb(&checked[..12]) != u16::from_le_bytes([raw[6], raw[7]])
        || crc16_usb(&checked[12..16 + payload_len]) != u16::from_le_bytes([raw[14], raw[15]])
    {
        return None;
    }
    Some(&raw[16..16 + payload_len])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracking_matches_hardware_commands_and_status() {
        for (enabled, category) in [(false, 0), (true, 2)] {
            let payload = tracking_payload(enabled);
            assert_eq!(&payload[..4], &[0x16, 2, category, 0]);
            assert!(payload[4..].iter().all(|&b| b == 0));
            let mut status = [0; FRAME_LEN];
            status[0] = 0x2e;
            status[0x18] = category;
            assert_eq!(decode_tracking(&status).unwrap(), enabled);
            assert_eq!(decode_ai(&status).unwrap(), (category, 0));
        }
    }

    #[test]
    fn tracking_rejects_invalid_status() {
        let mut status = [0; FRAME_LEN];
        assert!(decode_tracking(&status).is_err());
        status[0] = 0x2e;
        for len in 0..FRAME_LEN {
            assert!(decode_tracking(&status[..len]).is_err());
        }
        status[0x18] = 6;
        assert!(decode_tracking(&status).is_err());
        status[0x18] = 2;
        status[0x1c] = 5;
        assert!(decode_tracking(&status).is_err());
        status[0x1c] = 1;
        assert_eq!(decode_ai(&status).unwrap(), (2, 1));
    }

    #[test]
    fn serial_request_matches_public_protocol_vector() {
        // Independently calculated from obsbot-tiny3-linux's frame and CRC rules.
        let expected = hex_frame("aa0134120c0054d60a0dc818");
        assert_eq!(build_serial_request(0x1234), expected);
    }

    #[test]
    fn power_requests_match_public_protocol_vectors() {
        assert_eq!(
            build_power_request(0x00a5, false),
            hex_frame("aa25a5000c005fef0a02c2a00400be0700000000")
        );
        assert_eq!(
            build_power_request(0x0042, true),
            hex_frame("aa2542000c00ea630a02c2a00400bffb01000000")
        );
    }

    #[test]
    fn sleep_state_requires_full_status_and_known_value() {
        let mut status = [0; FRAME_LEN];
        assert!(asleep(&status).is_err());
        status[0] = 0x2e;
        assert!(!asleep(&status).unwrap());
        status[2] = 1;
        assert!(asleep(&status).unwrap());
        status[2] = 2;
        assert!(asleep(&status).is_err());
        for len in 0..FRAME_LEN {
            assert!(asleep(&status[..len]).is_err());
        }
        let long = [0x2e; FRAME_LEN + 1];
        assert!(asleep(&long).is_err());
    }

    #[test]
    fn serial_reply_validates_header_payload_and_transaction() {
        // Reference-generated reply, using the Lite serial published in PROTOCOL.md.
        let reply = hex_frame("aa2934120c0065c30d0ac8180e001d46524d4f5755484933313131504c4e");
        assert_eq!(
            parse_serial_reply(&reply, 0x1234),
            Some(&b"RMOWUHI3111PLN"[..])
        );
        assert!(parse_serial_reply(&reply, 0x1235).is_none());
        for len in 0..FRAME_LEN {
            assert!(parse_serial_reply(&reply[..len], 0x1234).is_none());
        }
        for offset in 0..30 {
            let mut corrupt = reply;
            corrupt[offset] ^= 1;
            assert!(
                parse_serial_reply(&corrupt, 0x1234).is_none(),
                "offset {offset}"
            );
        }
        assert!(parse_serial_reply(&build_serial_request(0x1234), 0x1234).is_none());
    }

    fn hex_frame(hex: &str) -> [u8; FRAME_LEN] {
        let mut frame = [0; FRAME_LEN];
        for (i, byte) in frame.iter_mut().take(hex.len() / 2).enumerate() {
            *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
        }
        frame
    }
}
