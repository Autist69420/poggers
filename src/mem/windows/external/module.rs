use std::{cell::RefCell, os::raw::c_void, rc::Rc, sync::Arc};

use windows::Win32::{
    Foundation::{CloseHandle, HANDLE, HINSTANCE},
    System::{
        Diagnostics::{
            Debug,
            ToolHelp::{
                CreateToolhelp32Snapshot, Module32First, Module32Next, MODULEENTRY32,
                TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32,
            },
        },
        Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_NOACCESS},
    },
};

use crate::mem::sigscan::SigScan;

use super::process::Process;
use anyhow::{Context, Result};
use thiserror::Error;


/// A module in a process.
#[derive(Debug)]
pub struct Module<'a> {
    pub(crate) process: &'a Process,
    pub(crate) base_address: usize,
    pub(crate) size: usize,
    pub(crate) name: String,
    pub(crate) handle: HINSTANCE,
}

impl<'a> Module<'a> {
    /// create a new module object from a process and a module name.
    /// # Arguments
    /// * `name` - The name of the module to find.
    /// * `process` - The process to find the module in.
    /// # Example
    /// ```
    /// use poggers::mem::process::Process;
    /// use poggers::mem::module::Module;
    /// let process = Process::new("notepad.exe").unwrap();
    /// let module = Module::new("user32.dll", &process).unwrap();
    /// ```
    /// # Errors
    /// * [`ModuleError::NoModuleFound`] - The module was not found in the process.
    /// * [`ModuleError::UnableToOpenHandle`] - The module handle could not be retrieved.
    pub fn new(name: &str, proc: &'a Process) -> Result<Self> {
        let mut me: MODULEENTRY32 = Default::default();
        me.dwSize = std::mem::size_of::<MODULEENTRY32>() as u32;

        let snap_handl =
            unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, proc.pid) }
                .or(Err(ModuleError::UnableToOpenHandle))?;

        let mut result: Option<String> = None;

        let le_poggier = |m: &MODULEENTRY32| {
            let f = m.szModule.iter().map(|x| x.0).collect::<Vec<u8>>();
            let x_name = String::from_utf8_lossy(&f);
            let x_name = x_name.trim_matches('\x00');
            return (x_name == name, x_name.to_string());
        };

        let hres = unsafe { Module32First(snap_handl, &mut me) };
        if !hres.as_bool() {
            return Err(ModuleError::UnableToOpenHandle.into());
        }
        let (is_poggier, mod_name) = le_poggier(&me);
        if is_poggier {
            result.replace(mod_name);
        } else {
            while unsafe { Module32Next(snap_handl, &mut me) }.as_bool()
                && result.is_none()
                && me.th32ProcessID != 0
            {
                let (is_ok, mod_name) = le_poggier(&me);
                if is_ok {
                    result = Some(mod_name);
                    break;
                } else {
                    continue;
                }
            }
        }
        if result.is_none() {
            return Err(ModuleError::NoModuleFound(name.to_string()).into());
        }
        Ok(Self {
            process: proc,
            base_address: me.modBaseAddr as usize,
            size: me.modBaseSize as usize,
            name: result.unwrap(),
            handle: me.hModule,
        })
    }

    /// Pattern scan this module to find an address
    /// # Arguments
    /// * `pattern` - The pattern to scan for (IDA Style).
    /// # Example
    /// ```
    /// use poggers::mem::process::Process;
    /// use poggers::mem::module::Module;
    /// let process = Process::new("notepad.exe").unwrap();
    /// let module = Module::new("user32.dll", &process).unwrap();
    /// let address = module.pattern_scan("48 8B 05 ? ? ? ? 48 8B 88 ? ? ? ? 48 85 C9 74 0A").unwrap();
    /// ```
    /// 
    pub fn scan_virtual(&self, pattern: &str) -> Option<usize> {
        let mut mem_info: MEMORY_BASIC_INFORMATION = Default::default();
        mem_info.RegionSize = 0x4096;

        println!("{} -> {}", self.base_address, self.size);

        let mut addr = self.base_address;

        loop { 
            if addr >= self.base_address + self.size {
                break;
            }

            let worky = unsafe {
                VirtualQueryEx(
                    self.process.handl,
                    addr as *const c_void,
                    &mut mem_info,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if mem_info.State != MEM_COMMIT || mem_info.Protect == PAGE_NOACCESS {
                addr += mem_info.RegionSize as usize;
                continue;
            }

            let page = self
                .process
                .read_sized(addr, mem_info.RegionSize - 1)
                .ok()?;

            let scan_res = self.scan_batch(pattern, &page);

            if let Some(result) = scan_res {
                println!("Found pattern at {:#x}", scan_res.unwrap());
                return Some(addr + result);
            }
            addr += mem_info.RegionSize as usize;
        }
        None
    }

    pub fn get_relative(&self, addr: usize) -> usize {
        addr - self.base_address
    }

    pub fn resolve_relative_ptr(&self, addr: usize, offset: u32) -> Result<usize> {
        let real_offset = self.process.read::<u32>(addr + offset as usize)?;
        println!("Real offset: {:X?}", real_offset);
        Ok(self.base_address + (self.get_relative(addr) + real_offset as usize))
    }
}

#[derive(Debug, Error)]
pub enum ModuleError {
    #[error("Unable to open handle")]
    UnableToOpenHandle,
    #[error("No module found for {0}")]
    NoModuleFound(String),
}

impl<'a> SigScan for Module<'a> {
    fn read<T: Default>(&self, addr: usize) -> Result<T> {
        self.process.read::<T>(addr)
    }
}

// impl<'a> Drop for Module<'a> { // we don't own the handle
//     fn drop(&mut self) {
//         unsafe { CloseHandle(std::transmuteself.handle); }
//     }
// }

mod tests {
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
    };

    use super::*;
    #[test]
    fn read() {
        let proc = Command::new("./test-utils/rw-test.exe")
            .stdout(Stdio::piped())
            .spawn()
            .expect("bruh");
        let proc_id = proc.id();
        let l = BufReader::new(proc.stdout.unwrap());
        let mut lines = l.lines();
        let current_val = lines.next().unwrap().unwrap();

        println!("Current value: {} -> pid = {proc_id}", current_val);
        let xproc = Process::new_from_pid(proc_id).unwrap();

        let base_mod = xproc.get_base_module().unwrap();

        println!(
            "predicted = {:X} | b = {:X}",
            base_mod.base_address + 0x43000,
            base_mod.base_address
        );

        let offset = base_mod.base_address + 0x43000;

        let read_val = xproc.read::<u32>(offset).unwrap();

        assert_eq!(current_val.parse::<u32>().unwrap(), read_val);
    }
    #[test]
    fn write() {
        let proc = Command::new("./test-utils/rw-test.exe")
            .stdout(Stdio::piped())
            .spawn()
            .expect("bruh");
        let proc_id = proc.id();
        let l = BufReader::new(proc.stdout.unwrap());
        let mut lines = l.lines();
        let current_val = lines.next().unwrap().unwrap();

        println!("Current value: {} -> pid = {proc_id}", current_val);
        let xproc = Process::new_from_pid(proc_id).unwrap();

        let base_mod = xproc.get_base_module().unwrap();

        println!(
            "predicted = {:X} | b = {:X}",
            base_mod.base_address + 0x43000,
            base_mod.base_address
        );

        let offset = base_mod.base_address + 0x43000;

        let read_val = xproc.read::<u32>(offset).unwrap();

        assert_eq!(current_val.parse::<u32>().unwrap(), read_val);

        xproc.write(offset, &9832472u32).unwrap();

        let current_val = lines.next().unwrap().unwrap();

        assert_eq!(current_val.parse::<u32>().unwrap(), 9832472u32);
    }
    #[test]
    fn sig() {
        let proc = Command::new("./test-utils/rw-test.exe")
            .stdout(Stdio::piped())
            .spawn()
            .expect("bruh");
        let proc_id = proc.id();
        let l = BufReader::new(proc.stdout.unwrap());
        let mut lines = l.lines();
        let current_val = lines.next().unwrap().unwrap();

        println!("Current value: {} -> pid = {proc_id}", current_val);
        let xproc = Process::new_from_pid(proc_id).unwrap();

        let base_mod = xproc.get_base_module().unwrap();

        println!(
            "predicted = {:X} | b = {:X}",
            base_mod.base_address + 0x43000,
            base_mod.base_address
        );

        let offset = base_mod.scan_virtual("F3 48 0F 2A C0").unwrap();
    }
}
