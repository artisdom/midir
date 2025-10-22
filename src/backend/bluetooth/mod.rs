use std::sync::{Arc, Mutex};
use std::thread::{Builder, JoinHandle};
use std::time::{Duration, Instant};

use crate::errors::*;
use crate::Ignore;

use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, PeripheralProperties, ScanFilter,
    WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral, PeripheralId};
use futures::StreamExt;
use tokio::runtime::Runtime;
use tokio::sync::watch;
use uuid::Uuid;

const MIDI_SERVICE_UUID: Uuid = Uuid::from_u128(0x03B8_0E5A_EDE8_4B33_A751_6CE3_4EC4_C700);
const MIDI_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x7772_E5DB_3868_4112_A1A9_F266_9D10_6BF3);

const RUNTIME_ERROR: &str = "failed to create Bluetooth runtime";
const MANAGER_ERROR: &str = "failed to access Bluetooth manager";
const ADAPTER_ERROR: &str = "failed to query Bluetooth adapters";
const SCAN_ERROR: &str = "failed to start Bluetooth scan";
const PERIPHERAL_ERROR: &str = "Bluetooth MIDI device no longer available";
const CONNECT_ERROR: &str = "failed to connect to Bluetooth MIDI device";
const DISCOVERY_ERROR: &str = "failed to discover Bluetooth MIDI services";
const CHARACTERISTIC_ERROR: &str = "Bluetooth MIDI characteristic not available";
const SUBSCRIBE_ERROR: &str = "failed to subscribe to Bluetooth MIDI notifications";
const NOTIFICATION_ERROR: &str = "failed to receive Bluetooth MIDI notifications";
const WRITE_ERROR: &str = "failed to send Bluetooth MIDI data";

#[derive(Clone)]
struct BluetoothPort {
    adapter_index: usize,
    peripheral_id: PeripheralId,
    name: String,
}

impl PartialEq for BluetoothPort {
    fn eq(&self, other: &Self) -> bool {
        self.adapter_index == other.adapter_index && self.peripheral_id == other.peripheral_id
    }
}

impl Eq for BluetoothPort {}

impl BluetoothPort {
    fn stable_id(&self) -> String {
        format!(
            "{}:{}",
            self.adapter_index,
            format_peripheral_id(&self.peripheral_id)
        )
    }
}

#[derive(Clone, PartialEq)]
pub struct MidiInputPort {
    inner: BluetoothPort,
}

impl MidiInputPort {
    pub fn id(&self) -> String {
        self.inner.stable_id()
    }
}

#[derive(Clone, PartialEq)]
pub struct MidiOutputPort {
    inner: BluetoothPort,
}

impl MidiOutputPort {
    pub fn id(&self) -> String {
        self.inner.stable_id()
    }
}

pub struct MidiInput {
    client_name: String,
    ignore_flags: Ignore,
}

pub struct MidiOutput {
    client_name: String,
}

pub struct MidiInputConnection<T: 'static> {
    client_name: String,
    ignore_flags: Ignore,
    stop_tx: Option<watch::Sender<bool>>,
    thread: Option<JoinHandle<HandlerThreadResult<T>>>,
}

pub struct MidiOutputConnection {
    client_name: String,
    runtime: Runtime,
    peripheral: Peripheral,
    characteristic: Characteristic,
}

struct HandlerData<T> {
    ignore_flags: Ignore,
    callback: Box<dyn FnMut(u64, &[u8], &mut T) + Send>,
    user_data: Option<T>,
    parser_state: ParserState,
}

struct HandlerThreadResult<T> {
    user_data: Option<T>,
}

#[derive(Default)]
struct ParserState {
    running_status: Option<u8>,
    sysex_buffer: Option<Vec<u8>>,
}

impl ParserState {
    fn new() -> Self {
        Self::default()
    }
}

impl MidiInput {
    pub fn new(client_name: &str) -> Result<Self, InitError> {
        ensure_bluetooth_manager()?;
        Ok(MidiInput {
            client_name: client_name.to_string(),
            ignore_flags: Ignore::None,
        })
    }

    pub fn ignore(&mut self, flags: Ignore) {
        self.ignore_flags = flags;
    }

    pub(crate) fn ports_internal(&self) -> Vec<crate::common::MidiInputPort> {
        match discover_ports_sync() {
            Ok(ports) => ports
                .into_iter()
                .map(|port| crate::common::MidiInputPort {
                    imp: MidiInputPort { inner: port },
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn port_count(&self) -> usize {
        self.ports_internal().len()
    }

    pub fn port_name(&self, port: &MidiInputPort) -> Result<String, PortInfoError> {
        Ok(port.inner.name.clone())
    }

    pub fn connect<F, T: Send + 'static>(
        self,
        port: &MidiInputPort,
        _port_name: &str,
        callback: F,
        data: T,
    ) -> Result<MidiInputConnection<T>, ConnectError<MidiInput>>
    where
        F: FnMut(u64, &[u8], &mut T) + Send + 'static,
    {
        let handler_data = Arc::new(Mutex::new(HandlerData {
            ignore_flags: self.ignore_flags,
            callback: Box::new(callback),
            user_data: Some(data),
            parser_state: ParserState::new(),
        }));

        let (init_tx, init_rx) = std::sync::mpsc::channel::<Result<(), &'static str>>();
        let (stop_tx, stop_rx) = watch::channel(false);
        let port_inner = port.inner.clone();
        let handler_clone = handler_data.clone();

        let thread_builder = Builder::new();
        let thread = match thread_builder
            .name("midir-bluetooth-in".into())
            .spawn(move || {
                let runtime = match Runtime::new() {
                    Ok(rt) => rt,
                    Err(_) => {
                        let _ = init_tx.send(Err(RUNTIME_ERROR));
                        let mut handler = handler_clone.lock().unwrap();
                        return HandlerThreadResult {
                            user_data: handler.user_data.take(),
                        };
                    }
                };

                let init_result = runtime.block_on(run_input_loop(
                    port_inner,
                    handler_clone.clone(),
                    stop_rx,
                    init_tx.clone(),
                ));

                if let Err(msg) = init_result {
                    let _ = init_tx.send(Err(msg));
                }

                let mut handler = handler_clone.lock().unwrap();
                HandlerThreadResult {
                    user_data: handler.user_data.take(),
                }
            }) {
            Ok(handle) => handle,
            Err(_) => return Err(ConnectError::other(RUNTIME_ERROR, self)),
        };

        match init_rx.recv() {
            Ok(Ok(())) => Ok(MidiInputConnection {
                client_name: self.client_name,
                ignore_flags: self.ignore_flags,
                stop_tx: Some(stop_tx),
                thread: Some(thread),
            }),
            Ok(Err(msg)) => {
                let _ = stop_tx.send(true);
                let _ = thread.join();
                Err(ConnectError::other(msg, self))
            }
            Err(_) => Err(ConnectError::other(NOTIFICATION_ERROR, self)),
        }
    }

    #[cfg(unix)]
    pub fn create_virtual<F, T: Send>(
        self,
        _port_name: &str,
        _callback: F,
        _data: T,
    ) -> Result<MidiInputConnection<T>, ConnectError<Self>>
    where
        F: FnMut(u64, &[u8], &mut T) + Send + 'static,
    {
        Err(ConnectError::other(
            "virtual Bluetooth MIDI ports are not supported",
            self,
        ))
    }
}

impl<T> MidiInputConnection<T> {
    pub fn close(mut self) -> (MidiInput, T) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(true);
        }

        let data = self
            .thread
            .take()
            .and_then(|handle| handle.join().ok())
            .and_then(|result| result.user_data)
            .expect("Bluetooth MIDI input handler failed");

        (
            MidiInput {
                client_name: self.client_name,
                ignore_flags: self.ignore_flags,
            },
            data,
        )
    }
}

impl MidiOutput {
    pub fn new(client_name: &str) -> Result<Self, InitError> {
        ensure_bluetooth_manager()?;
        Ok(MidiOutput {
            client_name: client_name.to_string(),
        })
    }

    pub(crate) fn ports_internal(&self) -> Vec<crate::common::MidiOutputPort> {
        match discover_ports_sync() {
            Ok(ports) => ports
                .into_iter()
                .map(|port| crate::common::MidiOutputPort {
                    imp: MidiOutputPort { inner: port },
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn port_count(&self) -> usize {
        self.ports_internal().len()
    }

    pub fn port_name(&self, port: &MidiOutputPort) -> Result<String, PortInfoError> {
        Ok(port.inner.name.clone())
    }

    pub fn connect(
        self,
        port: &MidiOutputPort,
        _port_name: &str,
    ) -> Result<MidiOutputConnection, ConnectError<MidiOutput>> {
        let runtime = match Runtime::new() {
            Ok(rt) => rt,
            Err(_) => return Err(ConnectError::other(RUNTIME_ERROR, self)),
        };

        match runtime.block_on(connect_output_port(port.inner.clone())) {
            Ok((peripheral, characteristic)) => Ok(MidiOutputConnection {
                client_name: self.client_name.clone(),
                runtime,
                peripheral,
                characteristic,
            }),
            Err(msg) => Err(ConnectError::other(msg, self)),
        }
    }

    #[cfg(unix)]
    pub fn create_virtual(
        self,
        _port_name: &str,
    ) -> Result<MidiOutputConnection, ConnectError<Self>> {
        Err(ConnectError::other(
            "virtual Bluetooth MIDI ports are not supported",
            self,
        ))
    }
}

impl MidiOutputConnection {
    pub fn close(self) -> MidiOutput {
        let _ = self.runtime.block_on(async {
            self.peripheral.unsubscribe(&self.characteristic).await.ok();
            self.peripheral.disconnect().await.ok();
        });

        MidiOutput {
            client_name: self.client_name,
        }
    }

    pub fn send(&mut self, message: &[u8]) -> Result<(), SendError> {
        if message.is_empty() {
            return Ok(());
        }

        let packets = encode_ble_midi_packets(message);
        for packet in packets {
            self.runtime
                .block_on(self.peripheral.write(
                    &self.characteristic,
                    &packet,
                    WriteType::WithoutResponse,
                ))
                .map_err(|_| SendError::Other(WRITE_ERROR))?;
        }

        Ok(())
    }
}

impl Clone for MidiOutput {
    fn clone(&self) -> Self {
        MidiOutput {
            client_name: self.client_name.clone(),
        }
    }
}

fn ensure_bluetooth_manager() -> Result<(), InitError> {
    let runtime = Runtime::new().map_err(|_| InitError)?;
    let result = runtime.block_on(async {
        let _ = Manager::new().await.map_err(|_| InitError)?;
        Ok::<(), InitError>(())
    });
    drop(runtime);
    result
}

fn discover_ports_sync() -> Result<Vec<BluetoothPort>, &'static str> {
    let runtime = Runtime::new().map_err(|_| RUNTIME_ERROR)?;
    let ports = runtime.block_on(discover_ports_async());
    drop(runtime);
    ports
}

async fn discover_ports_async() -> Result<Vec<BluetoothPort>, &'static str> {
    let manager = Manager::new().await.map_err(|_| MANAGER_ERROR)?;
    let adapters = manager.adapters().await.map_err(|_| ADAPTER_ERROR)?;

    let mut ports = Vec::new();
    for (idx, adapter) in adapters.into_iter().enumerate() {
        adapter
            .start_scan(ScanFilter::default())
            .await
            .map_err(|_| SCAN_ERROR)?;
        tokio::time::sleep(Duration::from_millis(400)).await;
        let peripherals = adapter.peripherals().await.map_err(|_| ADAPTER_ERROR)?;
        for peripheral in peripherals {
            if let Ok(Some(properties)) = peripheral.properties().await {
                if !is_midi_device(&properties) {
                    continue;
                }
                let name = properties
                    .local_name
                    .clone()
                    .unwrap_or_else(|| "Bluetooth MIDI".to_string());
                ports.push(BluetoothPort {
                    adapter_index: idx,
                    peripheral_id: peripheral.id(),
                    name,
                });
            }
        }
        let _ = adapter.stop_scan().await;
    }

    Ok(ports)
}

async fn run_input_loop<T: Send + 'static>(
    port: BluetoothPort,
    handler: Arc<Mutex<HandlerData<T>>>,
    mut stop_rx: watch::Receiver<bool>,
    init_tx: std::sync::mpsc::Sender<Result<(), &'static str>>,
) -> Result<(), &'static str> {
    let manager = Manager::new().await.map_err(|_| MANAGER_ERROR)?;
    let mut adapters = manager.adapters().await.map_err(|_| ADAPTER_ERROR)?;
    let adapter = adapters
        .get_mut(port.adapter_index)
        .ok_or(PERIPHERAL_ERROR)?
        .clone();

    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|_| SCAN_ERROR)?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    let peripheral = find_peripheral(&adapter, &port.peripheral_id).await?;
    let _ = adapter.stop_scan().await;

    if !peripheral.is_connected().await.map_err(|_| CONNECT_ERROR)? {
        peripheral.connect().await.map_err(|_| CONNECT_ERROR)?;
    }

    peripheral
        .discover_services()
        .await
        .map_err(|_| DISCOVERY_ERROR)?;
    let characteristic = peripheral
        .characteristics()
        .into_iter()
        .find(|characteristic| characteristic.uuid == MIDI_CHARACTERISTIC_UUID)
        .ok_or(CHARACTERISTIC_ERROR)?;
    peripheral
        .subscribe(&characteristic)
        .await
        .map_err(|_| SUBSCRIBE_ERROR)?;

    let mut notifications = peripheral
        .notifications()
        .await
        .map_err(|_| NOTIFICATION_ERROR)?;

    let _ = init_tx.send(Ok(()));
    let start = Instant::now();

    loop {
        tokio::select! {
            changed = stop_rx.changed() => {
                if changed.is_ok() && *stop_rx.borrow() {
                    break;
                }
            }
            notification = notifications.next() => {
                match notification {
                    Some(value) => process_notification(&handler, &value.value, start),
                    None => break,
                }
            }
        }
    }

    peripheral.unsubscribe(&characteristic).await.ok();
    peripheral.disconnect().await.ok();

    Ok(())
}

async fn find_peripheral(adapter: &Adapter, id: &PeripheralId) -> Result<Peripheral, &'static str> {
    let peripherals = adapter.peripherals().await.map_err(|_| ADAPTER_ERROR)?;
    peripherals
        .into_iter()
        .find(|p| p.id() == *id)
        .ok_or(PERIPHERAL_ERROR)
}

async fn connect_output_port(
    port: BluetoothPort,
) -> Result<(Peripheral, Characteristic), &'static str> {
    let manager = Manager::new().await.map_err(|_| MANAGER_ERROR)?;
    let mut adapters = manager.adapters().await.map_err(|_| ADAPTER_ERROR)?;
    let adapter = adapters
        .get_mut(port.adapter_index)
        .ok_or(PERIPHERAL_ERROR)?
        .clone();

    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|_| SCAN_ERROR)?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    let peripheral = find_peripheral(&adapter, &port.peripheral_id).await?;
    let _ = adapter.stop_scan().await;

    if !peripheral.is_connected().await.map_err(|_| CONNECT_ERROR)? {
        peripheral.connect().await.map_err(|_| CONNECT_ERROR)?;
    }

    peripheral
        .discover_services()
        .await
        .map_err(|_| DISCOVERY_ERROR)?;
    let characteristic = peripheral
        .characteristics()
        .into_iter()
        .find(|characteristic| characteristic.uuid == MIDI_CHARACTERISTIC_UUID)
        .ok_or(CHARACTERISTIC_ERROR)?;

    Ok((peripheral, characteristic))
}

fn process_notification<T>(handler: &Arc<Mutex<HandlerData<T>>>, payload: &[u8], start: Instant) {
    if payload.len() < 2 {
        return;
    }

    let mut handler = match handler.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let HandlerData {
        ignore_flags,
        callback,
        user_data,
        parser_state,
    } = &mut *handler;

    if let Some(data) = user_data.as_mut() {
        let messages = decode_ble_midi(payload, parser_state);
        let ignore = *ignore_flags;
        for message in messages {
            if message.is_empty() {
                continue;
            }
            if should_ignore(ignore, message[0]) {
                continue;
            }
            let timestamp = start.elapsed().as_micros() as u64;
            (callback)(timestamp, &message, data);
        }
    }
}

fn should_ignore(ignore_flags: Ignore, status: u8) -> bool {
    (status == 0xF0 && ignore_flags.contains(Ignore::Sysex))
        || (status == 0xF1 && ignore_flags.contains(Ignore::Time))
        || (status == 0xF8 && ignore_flags.contains(Ignore::Time))
        || (status == 0xFE && ignore_flags.contains(Ignore::ActiveSense))
}

fn decode_ble_midi(payload: &[u8], state: &mut ParserState) -> Vec<Vec<u8>> {
    let mut messages = Vec::new();
    let mut idx = 1; // skip packet header

    while idx < payload.len() {
        let byte = payload[idx];
        if byte & 0x80 == 0 {
            idx += 1;
            continue;
        }

        // timestamp byte
        idx += 1;
        if idx >= payload.len() {
            break;
        }

        if state.sysex_buffer.is_some() {
            let (next_idx, finished) = {
                let buffer = state.sysex_buffer.as_mut().unwrap();
                extend_sysex(buffer, payload, idx)
            };
            let progressed = next_idx != idx;
            idx = next_idx;
            if finished {
                if let Some(buffer) = state.sysex_buffer.take() {
                    messages.push(buffer);
                }
            }
            if progressed {
                continue;
            }
        }

        let mut status_byte = payload[idx];
        let has_status = status_byte & 0x80 != 0;

        if has_status {
            idx += 1;
        } else if let Some(status) = state.running_status {
            status_byte = status;
        } else {
            idx += 1;
            continue;
        }

        match status_byte {
            0xF0 => {
                let mut buffer = vec![0xF0];
                let (next_idx, finished) = extend_sysex(&mut buffer, payload, idx);
                idx = next_idx;
                if finished {
                    messages.push(buffer);
                    state.sysex_buffer = None;
                } else {
                    state.sysex_buffer = Some(buffer);
                }
                state.running_status = None;
            }
            0xF7 => {
                if let Some(mut buffer) = state.sysex_buffer.take() {
                    buffer.push(0xF7);
                    messages.push(buffer);
                } else {
                    messages.push(vec![0xF7]);
                }
                state.running_status = None;
            }
            status if status >= 0xF8 => {
                messages.push(vec![status]);
                state.running_status = None;
            }
            status => {
                let mut message = vec![status];
                let expected = expected_data_length(status);
                let mut data_bytes = 0usize;
                while idx < payload.len() {
                    let byte = payload[idx];
                    if byte & 0x80 != 0 {
                        break;
                    }
                    message.push(byte);
                    idx += 1;
                    data_bytes += 1;
                    if let Some(expected) = expected {
                        if data_bytes >= expected {
                            break;
                        }
                    }
                }
                if let Some(expected) = expected {
                    if data_bytes == expected {
                        messages.push(message);
                        if status < 0xF0 {
                            state.running_status = Some(status);
                        } else {
                            state.running_status = None;
                        }
                    }
                } else {
                    messages.push(message);
                    state.running_status = None;
                }
            }
        }
    }

    messages
}

fn extend_sysex(buffer: &mut Vec<u8>, payload: &[u8], mut idx: usize) -> (usize, bool) {
    let mut finished = false;
    while idx < payload.len() {
        let byte = payload[idx];
        if byte & 0x80 != 0 {
            if byte == 0xF7 {
                buffer.push(byte);
                idx += 1;
                finished = true;
            }
            break;
        } else {
            buffer.push(byte);
            idx += 1;
        }
    }
    (idx, finished)
}

fn expected_data_length(status: u8) -> Option<usize> {
    match status {
        0x80..=0xBF => Some(2),
        0xC0..=0xDF => Some(1),
        0xE0..=0xEF => Some(2),
        0xF1 => Some(1),
        0xF2 => Some(2),
        0xF3 => Some(1),
        0xF6 => Some(0),
        _ => None,
    }
}

fn encode_ble_midi_packets(message: &[u8]) -> Vec<Vec<u8>> {
    const MAX_PAYLOAD: usize = 18; // 20 byte MTU minus header/timestamp
    let mut packets = Vec::new();
    if message.is_empty() {
        return packets;
    }

    let mut offset = 0;
    while offset < message.len() {
        let end = (offset + MAX_PAYLOAD).min(message.len());
        let mut packet = vec![0x80, 0x80];
        packet.extend_from_slice(&message[offset..end]);
        packets.push(packet);
        offset = end;
    }

    packets
}

fn is_midi_device(properties: &PeripheralProperties) -> bool {
    if properties
        .services
        .iter()
        .any(|uuid| *uuid == MIDI_SERVICE_UUID)
    {
        return true;
    }
    false
}

fn format_peripheral_id(id: &PeripheralId) -> String {
    format!("{:?}", id)
}
