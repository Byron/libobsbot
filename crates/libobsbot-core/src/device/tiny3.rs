// SPDX-License-Identifier: GPL-3.0-only
//! Tiny 3 controls; evidence and calibration: doc/protocol/tiny3/README.md.

use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

use crate::devices::tiny3;
use crate::types::{AiMode, ProductType};
use crate::uvc::{self, UvcGet};
use crate::{Device, Error, Result};

const CONFIRM_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_DELAY: Duration = Duration::from_millis(50);
const WAKE_SETTLE_DELAY: Duration = Duration::from_secs(2);
static SEQUENCE: AtomicU16 = AtomicU16::new(0);

fn next_sequence() -> u16 {
    let pid = std::process::id().to_le_bytes();
    SEQUENCE
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(u16::from_le_bytes([pid[0], pid[1]]))
}

impl Device {
    fn require_tiny3(&self) -> Result<()> {
        if self.product_type() != ProductType::Tiny3 {
            return Err(Error::Unsupported(
                "this control is only verified on Tiny 3",
            ));
        }
        Ok(())
    }

    fn tiny3_status(&self) -> Result<[u8; tiny3::FRAME_LEN]> {
        let mut buf = [0; tiny3::FRAME_LEN];
        let n = self.transport.uvc_get(
            UvcGet::Cur,
            tiny3::XU_ENTITY_ID,
            tiny3::XU_SEL_STATUS,
            &mut buf,
        )?;
        if n != buf.len() {
            return Err(Error::BadResponse {
                selector: tiny3::XU_SEL_STATUS,
                bytes: buf[..n.min(buf.len())].to_vec(),
            });
        }
        tiny3::asleep(&buf)?;
        Ok(buf)
    }

    fn tracking_status(&self) -> Result<[u8; tiny3::FRAME_LEN]> {
        let deadline = Instant::now() + CONFIRM_TIMEOUT;
        loop {
            let status = self.tiny3_status()?;
            // Full Tiny 3 briefly reports category 6 while switching to ordinary tracking.
            if status[24] != 6 {
                tiny3::decode_ai(&status)?;
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            std::thread::sleep(POLL_DELAY);
        }
    }

    /// Read whether tracking is enabled from the camera, without caching.
    pub fn tracking(&self) -> Result<bool> {
        self.require_tiny3()?;
        let _transaction = self.transaction.lock().expect("transaction mutex poisoned");
        tiny3::decode_tracking(&self.tracking_status()?)
    }

    /// Enable ordinary person tracking or disable tracking; confirm readback.
    pub fn set_tracking(&self, on: bool) -> Result<bool> {
        self.require_tiny3()?;
        let _transaction = self.transaction.lock().expect("transaction mutex poisoned");
        self.write_tracking(on)
    }

    /// Read the actual camera state, toggle tracking, and confirm the result.
    pub fn toggle_tracking(&self) -> Result<bool> {
        self.require_tiny3()?;
        let _transaction = self.transaction.lock().expect("transaction mutex poisoned");
        let on = tiny3::decode_tracking(&self.tracking_status()?)?;
        self.write_tracking(!on)
    }

    fn write_tracking(&self, on: bool) -> Result<bool> {
        self.wake_for_control()?;
        self.transport.uvc_set(
            tiny3::XU_ENTITY_ID,
            tiny3::XU_SEL_STATUS,
            &tiny3::tracking_payload(on),
        )?;
        let deadline = Instant::now() + CONFIRM_TIMEOUT;
        loop {
            std::thread::sleep(POLL_DELAY);
            if tiny3::decode_ai(&self.tracking_status()?)? == (if on { 2 } else { 0 }, 0) {
                return Ok(on);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
        }
    }

    pub(super) fn tiny3_ai_mode(&self) -> Result<AiMode> {
        let _transaction = self.transaction.lock().expect("transaction mutex poisoned");
        let (category, _) = tiny3::decode_ai(&self.tracking_status()?)?;
        Ok(match category {
            0 => AiMode::None,
            1 => AiMode::Group,
            2 => AiMode::Human,
            3 => AiMode::Hand,
            4 => AiMode::WhiteBoard,
            5 => AiMode::Desk,
            _ => unreachable!("validated category"),
        })
    }

    pub(super) fn tiny3_serial(&self) -> Result<String> {
        let _transaction = self.transaction.lock().expect("transaction mutex poisoned");
        let seq = next_sequence();
        self.transport.uvc_set(
            tiny3::XU_ENTITY_ID,
            tiny3::XU_SEL_RPC,
            &tiny3::build_serial_request(seq),
        )?;
        let deadline = Instant::now() + CONFIRM_TIMEOUT;
        loop {
            std::thread::sleep(POLL_DELAY);
            let mut buf = [0; tiny3::FRAME_LEN];
            let n = self.transport.uvc_get(
                UvcGet::Cur,
                tiny3::XU_ENTITY_ID,
                tiny3::XU_SEL_RPC,
                &mut buf,
            )?;
            if let Some(payload) = tiny3::parse_serial_reply(&buf[..n.min(buf.len())], seq) {
                let serial = payload.trim_ascii_end();
                let serial = serial.split(|&b| b == 0).next().unwrap_or_default();
                if !serial.is_empty() && serial.iter().all(u8::is_ascii_graphic) {
                    return Ok(String::from_utf8(serial.to_vec()).expect("ASCII serial"));
                }
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
        }
    }

    /// Read whether the Tiny 3 is in standby, without waking it.
    pub fn standby(&self) -> Result<bool> {
        self.require_tiny3()?;
        let _transaction = self.transaction.lock().expect("transaction mutex poisoned");
        tiny3::asleep(&self.tiny3_status()?)
    }

    /// Put the Tiny 3 in standby or wake it, and confirm its power state.
    /// Returns `true` for confirmed standby and `false` for confirmed awake.
    /// A wake transition includes two seconds for firmware startup to settle.
    pub fn set_standby(&self, standby: bool) -> Result<bool> {
        self.require_tiny3()?;
        let _transaction = self.transaction.lock().expect("transaction mutex poisoned");
        self.write_standby(standby)
    }

    fn wake_for_control(&self) -> Result<()> {
        self.write_standby(false).map(|_| ())
    }

    fn write_standby(&self, standby: bool) -> Result<bool> {
        if tiny3::asleep(&self.tiny3_status()?)? == standby {
            return Ok(standby);
        }
        self.transport.uvc_set(
            tiny3::XU_ENTITY_ID,
            tiny3::XU_SEL_RPC,
            &tiny3::build_power_request(next_sequence(), standby),
        )?;
        let deadline = Instant::now() + CONFIRM_TIMEOUT;
        loop {
            std::thread::sleep(POLL_DELAY);
            if tiny3::asleep(&self.tiny3_status()?)? == standby {
                if !standby {
                    // Full Tiny 3 ignores sleep sent immediately after its awake flag clears.
                    std::thread::sleep(WAKE_SETTLE_DELAY);
                    if tiny3::asleep(&self.tiny3_status()?)? {
                        return Err(Error::Timeout);
                    }
                }
                return Ok(standby);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
        }
    }

    fn zoom_native(&self, req: UvcGet) -> Result<u16> {
        let mut buf = [0; 2];
        let n =
            self.transport
                .uvc_get(req, uvc::CAMERA_TERMINAL, uvc::ct::ZOOM_ABSOLUTE, &mut buf)?;
        if n != buf.len() {
            return Err(Error::BadResponse {
                selector: uvc::ct::ZOOM_ABSOLUTE,
                bytes: buf[..n.min(buf.len())].to_vec(),
            });
        }
        Ok(u16::from_le_bytes(buf))
    }

    fn zoom_scale(&self) -> Result<ZoomScale> {
        self.require_tiny3()?;
        ZoomScale::new(
            self.zoom_native(UvcGet::Min)?,
            self.zoom_native(UvcGet::Max)?,
            self.zoom_native(UvcGet::Res)?,
        )
    }

    /// Supported Tiny 3 magnification factors, queried from native bounds.
    pub fn zoom_factor_range(&self) -> Result<RangeInclusive<f64>> {
        let _transaction = self.transaction.lock().expect("transaction mutex poisoned");
        let scale = self.zoom_scale()?;
        Ok(scale.factor(scale.min)?..=scale.factor(scale.max)?)
    }

    /// Read Tiny 3 magnification. Native 0..100 maps to 1..4× in 0.03× steps.
    pub fn zoom_factor(&self) -> Result<f64> {
        let _transaction = self.transaction.lock().expect("transaction mutex poisoned");
        self.zoom_scale()?.factor(self.zoom_native(UvcGet::Cur)?)
    }

    /// Set Tiny 3 magnification, rounded to the nearest supported native step.
    /// Returns stable readback (which can settle up to 0.09× from the requested
    /// step on tested firmware). Unchanged or more distant results time out.
    pub fn set_zoom_factor(&self, factor: f64) -> Result<f64> {
        self.require_tiny3()?;
        if !factor.is_finite() {
            return Err(Error::OutOfRange);
        }
        let _transaction = self.transaction.lock().expect("transaction mutex poisoned");
        let scale = self.zoom_scale()?;
        let target = scale.native(factor)?;
        self.wake_for_control()?;
        let initial = self.zoom_native(UvcGet::Cur)?;
        self.transport.uvc_set(
            uvc::CAMERA_TERMINAL,
            uvc::ct::ZOOM_ABSOLUTE,
            &target.to_le_bytes(),
        )?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut previous = None;
        let mut stable_samples = 0;
        loop {
            std::thread::sleep(POLL_DELAY);
            let actual = self.zoom_native(UvcGet::Cur)?;
            scale.factor(actual)?;
            stable_samples = if previous == Some(actual) {
                stable_samples + 1
            } else {
                1
            };
            previous = Some(actual);
            // Measured firmware settling error is up to three units; an unchanged write is not confirmation.
            if stable_samples >= 10
                && (actual == target
                    || (actual.abs_diff(target) <= 3
                        && actual.abs_diff(target) < initial.abs_diff(target)))
            {
                return scale.factor(actual);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
        }
    }
}

struct ZoomScale {
    min: u16,
    max: u16,
    step: u16,
}

impl ZoomScale {
    fn new(min: u16, max: u16, step: u16) -> Result<Self> {
        // shortcut: only the calibrated Tiny 3 native scale is supported; remeasure other scales.
        if min > max || max > 100 || step == 0 || step > max - min || (max - min) % step != 0 {
            return Err(Error::Unsupported(
                "unrecognised Tiny 3 zoom range or resolution",
            ));
        }
        Ok(Self { min, max, step })
    }

    fn factor(&self, native: u16) -> Result<f64> {
        if !(self.min..=self.max).contains(&native) || (native - self.min) % self.step != 0 {
            return Err(Error::BadResponse {
                selector: uvc::ct::ZOOM_ABSOLUTE,
                bytes: native.to_le_bytes().to_vec(),
            });
        }
        Ok((100.0 + 3.0 * f64::from(native)) / 100.0)
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn native(&self, factor: f64) -> Result<u16> {
        if !factor.is_finite()
            || !(self.factor(self.min)?..=self.factor(self.max)?).contains(&factor)
        {
            return Err(Error::OutOfRange);
        }
        let steps =
            (((factor - 1.0) / 0.03 - f64::from(self.min)) / f64::from(self.step)).round() as u16;
        Ok(self.min + steps * self.step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{meet2_mock_info, ScriptedTransport};
    use std::sync::Arc;

    fn device(responses: Vec<Vec<u8>>) -> Device {
        let mut info = meet2_mock_info();
        info.product_type = ProductType::Tiny3;
        Device::new(
            info,
            Arc::new(ScriptedTransport::new(responses)),
            None,
            [0; 6],
        )
    }

    fn status(category: u8) -> Vec<u8> {
        let mut status = vec![0; 60];
        status[0] = 0x2e;
        status[24] = category;
        status
    }

    #[test]
    fn standby_confirms_both_transitions_and_preserves_tracking_and_zoom() {
        let mut awake = status(2);
        awake[4] = 17;
        let mut asleep = awake.clone();
        asleep[2] = 1;
        let transport = Arc::new(ScriptedTransport::new(vec![
            awake.clone(),
            awake.clone(),
            asleep.clone(),
            asleep.clone(),
            asleep,
            awake.clone(),
            awake.clone(),
            awake,
        ]));
        let mut info = meet2_mock_info();
        info.product_type = ProductType::Tiny3;
        let d = Device::new(info, transport.clone(), None, [0; 6]);
        assert!(d.set_standby(true).unwrap());
        assert!(d.standby().unwrap());
        assert!(!d.set_standby(false).unwrap());
        assert!(!d.standby().unwrap());
        let sets = transport.sets.lock().unwrap();
        assert_eq!(sets.len(), 2);
        for (set, value) in sets.iter().zip([1, 0]) {
            assert_eq!((set.0, set.1), (2, 2));
            assert_eq!(&set.2[10..12], &[0xc2, 0xa0]);
            assert_eq!(&set.2[16..20], &[value, 0, 0, 0]);
        }
    }

    #[test]
    fn already_in_standby_needs_no_write_and_invalid_reads_fail() {
        let mut asleep = status(0);
        asleep[2] = 1;
        let transport = Arc::new(ScriptedTransport::new(vec![asleep]));
        let mut info = meet2_mock_info();
        info.product_type = ProductType::Tiny3;
        let d = Device::new(info, transport.clone(), None, [0; 6]);
        assert!(d.set_standby(true).unwrap());
        assert_eq!(transport.sets.lock().unwrap().len(), 0);
        assert!(device(vec![vec![0x2e]]).set_standby(true).is_err());
        assert!(crate::testing::device_with_mock()
            .0
            .set_standby(true)
            .is_err());
    }

    #[test]
    fn ignored_standby_write_times_out_instead_of_reporting_success() {
        assert!(matches!(
            device(vec![status(0); 100]).set_standby(true),
            Err(Error::Timeout)
        ));
    }

    #[test]
    fn wake_rechecks_state_after_startup_settles() {
        let mut asleep = status(0);
        asleep[2] = 1;
        assert!(matches!(
            device(vec![asleep.clone(), status(0), asleep]).set_standby(false),
            Err(Error::Timeout)
        ));
    }

    #[test]
    fn zoom_conversion_and_quantization() {
        let scale = ZoomScale::new(0, 100, 1).unwrap();
        for (requested, native, achieved) in [
            (1.0, 0, 1.0),
            (1.5, 17, 1.51),
            (2.0, 33, 1.99),
            (2.5, 50, 2.5),
            (4.0, 100, 4.0),
        ] {
            assert_eq!(scale.native(requested).unwrap(), native);
            assert!((scale.factor(native).unwrap() - achieved).abs() < 1e-10);
        }
        for invalid in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            0.0,
            0.999,
            4.001,
        ] {
            assert!(matches!(scale.native(invalid), Err(Error::OutOfRange)));
        }
        assert_eq!(ZoomScale::new(0, 100, 2).unwrap().native(1.5).unwrap(), 16);
        for (min, max, step) in [
            (100, 0, 1),
            (0, 101, 1),
            (0, 100, 0),
            (0, 100, 3),
            (0, 0, 1),
        ] {
            assert!(ZoomScale::new(min, max, step).is_err());
        }
        assert!(scale.factor(101).is_err());
        assert!(ZoomScale::new(0, 100, 2).unwrap().factor(17).is_err());
    }

    #[test]
    fn zoom_waits_for_quantized_native_readback() {
        let mut responses = vec![
            vec![0, 0],
            vec![100, 0],
            vec![1, 0],
            status(0),
            vec![0, 0],
            vec![10, 0],
        ];
        responses.extend(vec![vec![17, 0]; 10]);
        assert!((device(responses).set_zoom_factor(1.5).unwrap() - 1.51).abs() < 1e-10);
        assert!(device(vec![vec![0]]).zoom_factor().is_err());
        assert!(device(vec![vec![0, 0], vec![100, 0], vec![1, 0], vec![17]])
            .zoom_factor()
            .is_err());
    }

    #[test]
    fn zoom_reports_stable_achieved_factor_after_small_firmware_overshoot() {
        let mut responses = vec![vec![0, 0], vec![100, 0], vec![1, 0], status(0), vec![0, 0]];
        responses.extend(vec![vec![19, 0]; 10]);
        let transport = Arc::new(ScriptedTransport::new(responses));
        let mut info = meet2_mock_info();
        info.product_type = ProductType::Tiny3;
        let d = Device::new(info, transport.clone(), None, [0; 6]);
        assert!((d.set_zoom_factor(1.5).unwrap() - 1.57).abs() < 1e-10);
        assert_eq!(
            *transport.sets.lock().unwrap(),
            vec![(1, 0x0b, vec![17, 0])]
        );
    }

    #[test]
    fn unchanged_zoom_is_not_mistaken_for_nearby_achieved_zoom() {
        let mut responses = vec![vec![0, 0], vec![100, 0], vec![1, 0], status(0)];
        responses.extend(vec![vec![16, 0]; 110]);
        assert!(matches!(
            device(responses).set_zoom_factor(1.5),
            Err(Error::Timeout)
        ));
    }

    #[test]
    fn tracking_off_wakes_a_sleeping_camera_and_waits_before_writing() {
        let mut asleep = status(2);
        asleep[2] = 1;
        let transport = Arc::new(ScriptedTransport::new(vec![
            asleep.clone(),
            asleep,
            status(2),
            status(2),
            status(0),
        ]));
        let mut info = meet2_mock_info();
        info.product_type = ProductType::Tiny3;
        let d = Device::new(info, transport.clone(), None, [0; 6]);
        assert!(!d.set_tracking(false).unwrap());
        let sets = transport.sets.lock().unwrap();
        assert_eq!(sets.len(), 2);
        assert_eq!((sets[0].0, sets[0].1), (2, 2));
        assert_eq!(&sets[0].2[10..12], &[0xc2, 0xa0]);
        assert_eq!((sets[1].0, sets[1].1), (2, 6));
        assert_eq!(&sets[1].2[..4], &[0x16, 2, 0, 0]);
    }

    #[test]
    fn toggle_reads_actual_state_each_time_and_confirms_changes() {
        let d = device(vec![
            status(0),
            status(0),
            status(6),
            status(2),
            status(2),
            status(2),
            status(2),
            status(0),
        ]);
        assert!(d.toggle_tracking().unwrap());
        assert!(!d.toggle_tracking().unwrap());
        assert!(device(vec![vec![0x2e]]).toggle_tracking().is_err());
    }

    #[test]
    fn new_tiny3_does_not_invent_meet2_firmware_or_start_a_poller() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut info = meet2_mock_info();
        info.product_type = ProductType::Tiny3;
        let d = Device::new(
            info,
            Arc::new(ScriptedTransport::new(vec![])),
            Some(tx),
            [0; 6],
        );
        assert_eq!(d.firmware_version(), "");
        assert!(d.poller.is_none());
        assert!(matches!(
            d.firmware_from_camera(),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn accepted_but_ignored_tracking_command_is_not_success() {
        let d = device(vec![status(2); 100]);
        assert!(matches!(d.set_tracking(false), Err(Error::Timeout)));
    }
}
