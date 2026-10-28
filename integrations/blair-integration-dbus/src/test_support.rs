use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
};

pub struct Bus {
    child: Child,
    address: String,
}

impl Default for Bus {
    fn default() -> Self {
        let child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut bus = Self {
            child,
            address: String::new(),
        };
        BufReader::new(bus.child.stdout.take().unwrap())
            .read_line(&mut bus.address)
            .unwrap();
        bus
    }
}

impl Bus {
    pub fn builder(&self) -> zbus::connection::Builder<'_> {
        zbus::connection::Builder::address(self.address.trim()).unwrap()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
