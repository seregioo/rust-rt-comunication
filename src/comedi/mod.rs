pub mod comedi_driver {
    pub const AREF_GROUND: u32 = crate::comedi::comedilib::AREF_GROUND;
    pub const AREF_COMMON: u32 = crate::comedi::comedilib::AREF_COMMON;
    pub const AREF_DIFF: u32 = crate::comedi::comedilib::AREF_DIFF;
    pub const AREF_OTHER: u32 = crate::comedi::comedilib::AREF_OTHER;

    pub trait DeviceDriver {
        fn open(&mut self) -> Result<(), String>;
        fn close(&mut self);
    }

    #[derive(Debug, Clone, Copy)]
    pub struct PortCalibration {
        pub slope: f64,
        pub intercept: f64,
    }

    impl PortCalibration {
        pub fn identity() -> Self {
            Self {
                slope: 1.0,
                intercept: 0.0,
            }
        }

        pub fn invert_for_output(&self, desired_value: f64) -> f64 {
            if self.slope.abs() <= f64::EPSILON {
                desired_value
            } else {
                (desired_value - self.intercept) / self.slope
            }
        }

        pub fn normalize_input(&self, measured_value: f64) -> f64 {
            if self.slope.abs() <= f64::EPSILON {
                measured_value
            } else {
                (measured_value - self.intercept) / self.slope
            }
        }
    }

    pub struct ComediDaq {
        input_port_names: Vec<String>,
        output_port_names: Vec<String>,

        device_path: String,
        ai_range_index: u32,
        ao_range_index: u32,
        ai_aref: u32,
        ao_aref: u32,
        pub ai_channels: Vec<(u32, u32)>,
        pub ao_channels: Vec<(u32, u32)>,

        input_values: HashMap<String, f64>,
        output_values: HashMap<String, f64>,

        is_open: bool,
        last_scan_devices: bool,
        last_scan_nonce: u64,
        active_inputs: Vec<bool>,
        active_outputs: Vec<bool>,
        ao_port_names: Vec<String>,
        ai_port_names: Vec<String>,
        ao_port_calibration: HashMap<String, PortCalibration>,
        ai_port_calibration: HashMap<String, PortCalibration>,
        ao_port_calibration_cache: Vec<PortCalibration>,
        ai_port_calibration_cache: Vec<PortCalibration>,
        ao_calibration: Vec<Option<(comedilib::comedi_range, comedilib::LsamplT)>>,
        ai_calibration: Vec<Option<(comedilib::comedi_range, comedilib::LsamplT)>>,
        no_device_detected: bool,
        dev: Option<std::ptr::NonNull<comedilib::comedi_t>>,
    }

    impl ComediDaq {
        fn normalize_device_path(path: &str) -> &str {
            if let Some(idx) = path.find("_subd") {
                &path[..idx]
            } else {
                path
            }
        }

        pub fn new() -> Self {
            let mut plugin = Self {
                input_port_names: Vec::new(),
                output_port_names: Vec::new(),
                device_path: "/dev/comedi0".to_string(),
                ai_range_index: 0,
                ao_range_index: 0,
                ai_aref: comedilib::AREF_GROUND,
                ao_aref: comedilib::AREF_GROUND,
                ai_channels: Vec::new(),
                ao_channels: Vec::new(),
                input_values: HashMap::new(),
                output_values: HashMap::new(),
                is_open: false,
                last_scan_devices: false,
                last_scan_nonce: 0,
                active_inputs: Vec::new(),
                active_outputs: Vec::new(),
                ao_port_names: Vec::new(),
                ai_port_names: Vec::new(),
                ao_port_calibration: HashMap::new(),
                ai_port_calibration: HashMap::new(),
                ao_port_calibration_cache: Vec::new(),
                ai_port_calibration_cache: Vec::new(),
                ao_calibration: Vec::new(),
                ai_calibration: Vec::new(),
                no_device_detected: false,
                dev: None,
            };

            plugin.auto_configure();
            plugin
        }

        pub fn set_data_config(
            &mut self,
            ai_range_index: u32,
            ao_range_index: u32,
            ai_aref: u32,
            ao_aref: u32,
        ) {
            self.ai_range_index = ai_range_index;
            self.ao_range_index = ao_range_index;
            self.ai_aref = ai_aref;
            self.ao_aref = ao_aref;
            if self.is_open {
                let _ = self.rebuild_calibration_cache();
            }
        }

        pub fn set_output_port_calibration(
            &mut self,
            calibration: HashMap<String, PortCalibration>,
        ) {
            self.ao_port_calibration = calibration;
            self.rebuild_port_calibration_cache();
        }

        pub fn set_input_port_calibration(
            &mut self,
            calibration: HashMap<String, PortCalibration>,
        ) {
            self.ai_port_calibration = calibration;
            self.rebuild_port_calibration_cache();
        }

        pub fn set_config(&mut self, device_path: String, scan_devices: bool, scan_nonce: u64) {
            let changed = self.device_path != device_path;
            if changed {
                self.device_path = device_path;
            }
            if changed
                || (scan_devices && !self.last_scan_devices)
                || scan_nonce != self.last_scan_nonce
            {
                self.auto_configure();
            }
            self.last_scan_devices = scan_devices;
            self.last_scan_nonce = scan_nonce;
        }

        pub fn set_input(&mut self, port_name: &str, value: f64) {
            self.input_values.insert(port_name.to_string(), value);
        }

        pub fn get_output(&self, port_name: &str) -> f64 {
            self.output_values.get(port_name).copied().unwrap_or(0.0)
        }

        pub fn is_open(&self) -> bool {
            self.is_open
        }

        pub fn set_active_ports(
            &mut self,
            input_ports: &std::collections::HashSet<String>,
            output_ports: &std::collections::HashSet<String>,
        ) {
            if self.active_inputs.len() != self.input_port_names.len() {
                self.active_inputs
                    .resize(self.input_port_names.len(), false);
            }
            if self.active_outputs.len() != self.output_port_names.len() {
                self.active_outputs
                    .resize(self.output_port_names.len(), false);
            }
            for (idx, name) in self.input_port_names.iter().enumerate() {
                self.active_inputs[idx] = input_ports.contains(name);
            }
            for (idx, name) in self.output_port_names.iter().enumerate() {
                self.active_outputs[idx] = output_ports.contains(name);
            }
            self.rebuild_port_calibration_cache();
        }

        fn auto_configure(&mut self) {
            let device_path = Self::normalize_device_path(&self.device_path);
            let Ok(dev) = (unsafe { comedilib::open(device_path) }) else {
                self.no_device_detected = true;
                self.ai_channels.clear();
                self.ao_channels.clear();
                self.input_port_names.clear();
                self.output_port_names.clear();
                return;
            };

            let mut ai = Vec::new();
            let mut ao = Vec::new();
            let n = unsafe { comedilib::get_n_subdevices(dev).unwrap_or(0) };
            for sd in 0..n {
                match unsafe { comedilib::get_subdevice_type(dev, sd) } {
                    Ok(t) if t == comedilib::SUBD_AI => {
                        let ch = unsafe { comedilib::get_n_channels(dev, sd).unwrap_or(0) };
                        for c in 0..ch {
                            ai.push((sd, c));
                        }
                    }
                    Ok(t) if t == comedilib::SUBD_AO => {
                        let ch = unsafe { comedilib::get_n_channels(dev, sd).unwrap_or(0) };
                        for c in 0..ch {
                            ao.push((sd, c));
                        }
                    }
                    _ => {}
                }
            }

            self.no_device_detected = false;
            self.ai_channels = ai;
            self.ao_channels = ao;
            self.input_port_names = self
                .ai_channels
                .iter()
                .map(|(_, ch)| format!("a{ch}"))
                .collect();
            self.output_port_names = self
                .ao_channels
                .iter()
                .map(|(_, ch)| format!("i{ch}"))
                .collect();
            self.active_inputs.resize(self.input_port_names.len(), false);
            self.active_outputs.resize(self.output_port_names.len(), false);
            self.rebuild_port_calibration_cache();
            unsafe { comedilib::close(dev) };
        }

        fn rebuild_port_calibration_cache(&mut self) {
            self.ao_port_calibration_cache = self
                .output_port_names
                .iter()
                .map(|port_name| {
                    self.ao_port_calibration
                        .get(port_name)
                        .copied()
                        .unwrap_or_else(PortCalibration::identity)
                })
                .collect();
            self.ai_port_calibration_cache = self
                .input_port_names
                .iter()
                .map(|port_name| {
                    self.ai_port_calibration
                        .get(port_name)
                        .copied()
                        .unwrap_or_else(PortCalibration::identity)
                })
                .collect();
        }

        fn rebuild_calibration_cache(&mut self) -> Result<(), String> {
            let Some(dev) = self.dev.as_ref() else {
                if self.no_device_detected {
                    if let Some(value) = self.output_values.get_mut("no device detected") {
                        *value = 1.0;
                    } else {
                        self.output_values
                            .insert("no device detected".to_string(), 1.0);
                    }
                }
                return Ok(());
            };
            let dev = dev.as_ptr();
            self.ao_calibration.clear();
            self.ao_calibration.reserve(self.ao_channels.len());
            for (sd, ch) in &self.ao_channels {
                let range = unsafe { comedilib::get_range(dev, *sd, *ch, self.ao_range_index) }?;
                let max = unsafe { comedilib::get_maxdata(dev, *sd, *ch) }?;
                self.ao_calibration.push(Some((range, max)));
            }
            self.ai_calibration.clear();
            self.ai_calibration.reserve(self.ai_channels.len());
            for (sd, ch) in &self.ai_channels {
                let range = unsafe { comedilib::get_range(dev, *sd, *ch, self.ai_range_index) }?;
                let max = unsafe { comedilib::get_maxdata(dev, *sd, *ch) }?;
                self.ai_calibration.push(Some((range, max)));
            }
            Ok(())
        }

        fn prepare_output_channel(&mut self, idx: usize) -> Result<(), String> {
            let Some((_sd, _ch)) = self.ao_channels.get(idx).copied() else {
                return Ok(());
            };
            let Some((_range, _max)) = self.ao_calibration.get(idx).and_then(|v| *v) else {
                return Ok(());
            };
            let _ = self
                .ao_port_calibration_cache
                .get(idx)
                .copied()
                .unwrap_or_else(PortCalibration::identity);
            Ok(())
        }

        fn prepare_input_channel(&mut self, idx: usize) -> Result<(), String> {
            let Some(dev) = self.dev.as_ref() else {
                return Ok(());
            };
            let Some((sd, ch)) = self.ai_channels.get(idx).copied() else {
                return Ok(());
            };
            let Some((range, max)) = self.ai_calibration.get(idx).and_then(|v| *v) else {
                return Ok(());
            };
            let port_name = match self.input_port_names.get(idx) {
                Some(name) => name.clone(),
                None => return Ok(()),
            };
            let calibration = self
                .ai_port_calibration_cache
                .get(idx)
                .copied()
                .unwrap_or_else(PortCalibration::identity);

            // Discarding the first conversion primes the channel mux after device open.
            let _ = unsafe { comedilib::read(dev.as_ptr(), sd, ch, self.ai_range_index, self.ai_aref) }?;
            let raw = unsafe { comedilib::read(dev.as_ptr(), sd, ch, self.ai_range_index, self.ai_aref) }?;
            let phys = unsafe { comedilib::to_phys(raw, &range, max) };
            self.output_values
                .insert(port_name, calibration.normalize_input(phys));
            Ok(())
        }

        fn prepare_active_channels(&mut self) -> Result<(), String> {
            let active_output_indexes: Vec<usize> = self
                .active_outputs
                .iter()
                .enumerate()
                .filter_map(|(idx, active)| active.then_some(idx))
                .collect();
            for idx in active_output_indexes {
                self.prepare_output_channel(idx)?;
            }

            let active_input_indexes: Vec<usize> = self
                .active_inputs
                .iter()
                .enumerate()
                .filter_map(|(idx, active)| active.then_some(idx))
                .collect();
            for idx in active_input_indexes {
                self.prepare_input_channel(idx)?;
            }

            Ok(())
        }

        pub fn try_open(&mut self) -> Result<(), String> {
            if self.is_open || self.no_device_detected {
                return Ok(());
            }

            <Self as DeviceDriver>::open(self)
        }

        pub fn read(&mut self) -> f64 {
            let Some(dev) = self.dev.as_ref() else {
                return 0.0;
            };
            let dev = dev.as_ptr();

            for (idx, (sd, ch)) in self.ai_channels.iter().enumerate() {
                if self.active_inputs.get(idx).copied() == Some(false) {
                    continue;
                }

                let raw = match unsafe {
                    comedilib::read(dev, *sd, *ch, self.ai_range_index, self.ai_aref)
                } {
                    Ok(raw) => raw,
                    Err(_) => continue,
                };
                let Some((range, max)) = self.ai_calibration.get(idx).and_then(|v| *v) else {
                    continue;
                };
                let phys = unsafe { comedilib::to_phys(raw, &range, max) };
                let calibration = self
                    .ai_port_calibration_cache
                    .get(idx)
                    .copied()
                    .unwrap_or_else(PortCalibration::identity);
                let normalized_phys = calibration.normalize_input(phys);
                if let Some(port_name) = self.input_port_names.get(idx) {
                    self.output_values
                        .insert(port_name.clone(), normalized_phys);
                }

                return normalized_phys;
            }
            0.0
        }

        pub fn read_average(&mut self, samples: usize) -> f64 {
            if samples == 0 {
                return self.read();
            }
            let mut total = 0.0;
            for _ in 0..samples {
                total += self.read();
            }
            total / samples as f64
        }

        pub fn write(&mut self, value: f64) {
            let Some(dev) = self.dev.as_ref() else {
                return;
            };
            let dev = dev.as_ptr();
            for (idx, (sd, ch)) in self.ao_channels.iter().enumerate() {
                if self.active_outputs.get(idx).copied() == Some(false) {
                    continue;
                }

                let Some((range, max)) = self.ao_calibration.get(idx).and_then(|v| *v) else {
                    continue;
                };
                let calibration = self
                    .ao_port_calibration_cache
                    .get(idx)
                    .copied()
                    .unwrap_or_else(PortCalibration::identity);
                let calibrated_value = calibration.invert_for_output(value);
                let raw = unsafe { comedilib::from_phys(calibrated_value, &range, max) };
                if unsafe {
                    comedilib::write(dev, *sd, *ch, self.ao_range_index, self.ao_aref, raw)
                }
                .is_err()
                {
                    continue;
                }
            }
        }
    }

    impl DeviceDriver for ComediDaq {
        fn open(&mut self) -> Result<(), String> {
            let device_path = Self::normalize_device_path(&self.device_path);
            let dev = unsafe { comedilib::open(device_path) }?;
            self.dev = std::ptr::NonNull::new(dev);
            if let Err(err) = self
                .rebuild_calibration_cache()
                .and_then(|_| self.prepare_active_channels())
            {
                if let Some(dev) = self.dev.take() {
                    unsafe { comedilib::close(dev.as_ptr()) };
                }
                self.ao_calibration.clear();
                self.ai_calibration.clear();
                self.is_open = false;
                return Err(err);
            }
            self.is_open = true;
            Ok(())
        }

        fn close(&mut self) {
            if let Some(dev) = self.dev.take() {
                unsafe { comedilib::close(dev.as_ptr()) };
            }
            self.ao_calibration.clear();
            self.ai_calibration.clear();
            self.is_open = false;
        }
    }
    use std::{
        collections::HashMap,
        ffi::{CStr, CString, c_uint},
    };

    use crate::comedi::comedilib::{
        self, LsamplT, comedi_data_read, comedi_data_write, comedi_from_phys, comedi_open,
        comedi_range, comedi_t, comedi_to_phys,
    };

    fn last_error() -> String {
        unsafe {
            let err = comedilib::comedi_errno();
            let msg = comedilib::comedi_strerror(err);
            if msg.is_null() {
                format!("comedi error {err}")
            } else {
                CStr::from_ptr(msg).to_string_lossy().to_string()
            }
        }
    }

    pub fn open(path: &str) -> Result<*mut comedi_t, String> {
        let cpath = CString::new(path).map_err(|_| "invalid device path".to_string())?;
        unsafe {
            let dev = comedi_open(cpath.as_ptr());
            if dev.is_null() {
                Err(last_error())
            } else {
                Ok(dev)
            }
        }
    }

    pub fn read(
        dev: *mut comedi_t,
        subd: u32,
        chan: u32,
        range: u32,
        aref: u32,
    ) -> Result<LsamplT, String> {
        let mut data: LsamplT = 0;
        unsafe {
            let res = comedi_data_read(
                dev,
                subd as c_uint,
                chan as c_uint,
                range as c_uint,
                aref as c_uint,
                &mut data,
            );
            if res < 0 { Err(last_error()) } else { Ok(data) }
        }
    }

    pub fn write(
        dev: *mut comedi_t,
        subd: u32,
        chan: u32,
        range: u32,
        aref: u32,
        data: LsamplT,
    ) -> Result<(), String> {
        unsafe {
            let res = comedi_data_write(
                dev,
                subd as c_uint,
                chan as c_uint,
                range as c_uint,
                aref as c_uint,
                data,
            );
            if res < 0 { Err(last_error()) } else { Ok(()) }
        }
    }
    pub fn from_phys(data: f64, range: &comedi_range, maxdata: LsamplT) -> LsamplT {
        unsafe { comedi_from_phys(data, range as *const comedi_range, maxdata) }
    }

    pub fn to_phys(data: LsamplT, range: &comedi_range, maxdata: LsamplT) -> f64 {
        unsafe { comedi_to_phys(data, range as *const comedi_range, maxdata) as f64 }
    }
}
mod comedilib {
    use libc::{c_char, c_double, c_int, c_uint};
    use std::ffi::{CStr, CString};

    #[repr(C)]
    pub struct comedi_t {
        _private: [u8; 0],
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    pub struct comedi_range {
        pub min: c_double,
        pub max: c_double,
        pub unit: c_uint,
    }

    pub type LsamplT = c_uint;

    pub const AREF_GROUND: c_uint = 0;
    pub const AREF_COMMON: c_uint = 1;
    pub const AREF_DIFF: c_uint = 2;
    pub const AREF_OTHER: c_uint = 3;

    pub const SUBD_AI: c_int = 1;
    pub const SUBD_AO: c_int = 2;

    #[link(name = "comedi")]
    unsafe extern "C" {
        pub fn comedi_open(fn_ptr: *const c_char) -> *mut comedi_t;
        pub fn comedi_close(dev: *mut comedi_t) -> c_int;
        pub fn comedi_errno() -> c_int;
        pub fn comedi_strerror(errnum: c_int) -> *const c_char;

        pub fn comedi_get_n_subdevices(dev: *mut comedi_t) -> c_int;
        pub fn comedi_get_subdevice_type(dev: *mut comedi_t, subdevice: c_uint) -> c_int;
        pub fn comedi_get_n_channels(dev: *mut comedi_t, subdevice: c_uint) -> c_int;

        pub fn comedi_get_range(
            dev: *mut comedi_t,
            subdevice: c_uint,
            chan: c_uint,
            range: c_uint,
        ) -> *mut comedi_range;
        pub fn comedi_get_maxdata(dev: *mut comedi_t, subdevice: c_uint, chan: c_uint) -> LsamplT;

        pub fn comedi_to_phys(
            data: LsamplT,
            rng: *const comedi_range,
            maxdata: LsamplT,
        ) -> c_double;
        pub fn comedi_from_phys(
            data: c_double,
            rng: *const comedi_range,
            maxdata: LsamplT,
        ) -> LsamplT;

        pub fn comedi_data_read(
            dev: *mut comedi_t,
            subd: c_uint,
            chan: c_uint,
            range: c_uint,
            aref: c_uint,
            data: *mut LsamplT,
        ) -> c_int;
        pub fn comedi_data_write(
            dev: *mut comedi_t,
            subd: c_uint,
            chan: c_uint,
            range: c_uint,
            aref: c_uint,
            data: LsamplT,
        ) -> c_int;
    }

    fn last_error() -> String {
        unsafe {
            let err = comedi_errno();
            let msg = comedi_strerror(err);
            if msg.is_null() {
                format!("comedi error {err}")
            } else {
                CStr::from_ptr(msg).to_string_lossy().to_string()
            }
        }
    }

    pub unsafe fn open(path: &str) -> Result<*mut comedi_t, String> {
        let cpath = CString::new(path).map_err(|_| "invalid device path".to_string())?;
        let dev = unsafe { comedi_open(cpath.as_ptr()) };
        if dev.is_null() {
            Err(last_error())
        } else {
            Ok(dev)
        }
    }

    pub unsafe fn close(dev: *mut comedi_t) {
        let _ = unsafe { comedi_close(dev) };
    }

    pub unsafe fn get_n_subdevices(dev: *mut comedi_t) -> Result<u32, String> {
        let n = unsafe { comedi_get_n_subdevices(dev) };
        if n < 0 {
            Err(last_error())
        } else {
            Ok(n as u32)
        }
    }

    pub unsafe fn get_subdevice_type(dev: *mut comedi_t, subd: u32) -> Result<i32, String> {
        let t = unsafe { comedi_get_subdevice_type(dev, subd as c_uint) };
        if t < 0 { Err(last_error()) } else { Ok(t) }
    }

    pub unsafe fn get_n_channels(dev: *mut comedi_t, subd: u32) -> Result<u32, String> {
        let n = unsafe { comedi_get_n_channels(dev, subd as c_uint) };
        if n < 0 {
            Err(last_error())
        } else {
            Ok(n as u32)
        }
    }

    pub unsafe fn get_range(
        dev: *mut comedi_t,
        subd: u32,
        chan: u32,
        range_index: u32,
    ) -> Result<comedi_range, String> {
        let ptr = unsafe {
            comedi_get_range(
                dev,
                subd as c_uint,
                chan as c_uint,
                range_index as c_uint,
            )
        };
        if ptr.is_null() {
            Err(last_error())
        } else {
            Ok(unsafe { *ptr })
        }
    }

    pub unsafe fn get_maxdata(dev: *mut comedi_t, subd: u32, chan: u32) -> Result<LsamplT, String> {
        let val = unsafe { comedi_get_maxdata(dev, subd as c_uint, chan as c_uint) };
        if val == 0 {
            let err = unsafe { comedi_errno() };
            if err != 0 { Err(last_error()) } else { Ok(val) }
        } else {
            Ok(val)
        }
    }

    pub unsafe fn to_phys(data: LsamplT, range: &comedi_range, maxdata: LsamplT) -> f64 {
        unsafe { comedi_to_phys(data, range as *const comedi_range, maxdata) as f64 }
    }

    pub unsafe fn from_phys(data: f64, range: &comedi_range, maxdata: LsamplT) -> LsamplT {
        unsafe { comedi_from_phys(data, range as *const comedi_range, maxdata) }
    }

    pub unsafe fn read(
        dev: *mut comedi_t,
        subd: u32,
        chan: u32,
        range: u32,
        aref: u32,
    ) -> Result<LsamplT, String> {
        let mut data: LsamplT = 0;
        let res = unsafe {
            comedi_data_read(
                dev,
                subd as c_uint,
                chan as c_uint,
                range as c_uint,
                aref as c_uint,
                &mut data,
            )
        };
        if res < 0 { Err(last_error()) } else { Ok(data) }
    }

    pub unsafe fn write(
        dev: *mut comedi_t,
        subd: u32,
        chan: u32,
        range: u32,
        aref: u32,
        data: LsamplT,
    ) -> Result<(), String> {
        let res = unsafe {
            comedi_data_write(
                dev,
                subd as c_uint,
                chan as c_uint,
                range as c_uint,
                aref as c_uint,
                data,
            )
        };
        if res < 0 { Err(last_error()) } else { Ok(()) }
    }
}
