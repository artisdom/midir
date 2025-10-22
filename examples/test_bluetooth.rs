#[cfg(not(feature = "bluetooth"))]
compile_error!("This example requires the `bluetooth` feature. Run with `--features bluetooth`.");

use std::error::Error;
use std::io::{stdin, stdout, Write};
use std::thread::sleep;
use std::time::Duration;

use midir::{Ignore, MidiInput, MidiInputPort, MidiOutput, MidiOutputConnection, MidiOutputPort};

fn main() {
    match run() {
        Ok(_) => (),
        Err(err) => println!("Error: {}", err),
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut midi_in = MidiInput::new("GZUT-MIDI")?;
    midi_in.ignore(Ignore::None);
    let midi_out = MidiOutput::new("GZUT-MIDI")?;

    let in_port = select_input_port(&midi_in)?;
    let out_port = select_output_port(&midi_out)?;

    println!("\nOpening Bluetooth connections …");
    let in_port_name = midi_in.port_name(&in_port)?;

    // Keep the input connection alive until the end of the scope.
    let _conn_in = midi_in.connect(
        &in_port,
        "midir-bluetooth-in",
        move |stamp, message, _| {
            println!("Incoming [{} µs]: {:?}", stamp, message);
        },
        (),
    )?;

    let mut conn_out = midi_out.connect(&out_port, "midir-bluetooth-out")?;
    println!(
        "Listening on '{}' and ready to send test notes. Press <enter> to begin playback …",
        in_port_name
    );

    let mut input = String::new();
    stdin().read_line(&mut input)?;

    play_test_pattern(&mut conn_out);

    println!("Playback done. Press <enter> to stop reading input …");
    input.clear();
    stdin().read_line(&mut input)?;

    println!("Closing Bluetooth connections");
    conn_out.close();
    println!("Connections closed");

    Ok(())
}

fn select_input_port(midi_in: &MidiInput) -> Result<MidiInputPort, Box<dyn Error>> {
    let in_ports = midi_in.ports();
    match in_ports.len() {
        0 => Err("no Bluetooth MIDI input ports found".into()),
        1 => {
            println!(
                "Using the only available input port: {}",
                midi_in.port_name(&in_ports[0])?
            );
            Ok(in_ports[0].clone())
        }
        _ => {
            println!("Available Bluetooth MIDI input ports:");
            for (i, port) in in_ports.iter().enumerate() {
                println!("{}: {}", i, midi_in.port_name(port)?);
            }
            print!("Select input port: ");
            stdout().flush()?;
            let mut input = String::new();
            stdin().read_line(&mut input)?;
            let selection = input.trim().parse::<usize>()?;
            in_ports
                .get(selection)
                .cloned()
                .ok_or_else(|| "invalid input port selected".into())
        }
    }
}

fn select_output_port(midi_out: &MidiOutput) -> Result<MidiOutputPort, Box<dyn Error>> {
    let out_ports = midi_out.ports();
    match out_ports.len() {
        0 => Err("no Bluetooth MIDI output ports found".into()),
        1 => {
            println!(
                "Using the only available output port: {}",
                midi_out.port_name(&out_ports[0])?
            );
            Ok(out_ports[0].clone())
        }
        _ => {
            println!("\nAvailable Bluetooth MIDI output ports:");
            for (i, port) in out_ports.iter().enumerate() {
                println!("{}: {}", i, midi_out.port_name(port)?);
            }
            print!("Select output port: ");
            stdout().flush()?;
            let mut input = String::new();
            stdin().read_line(&mut input)?;
            let selection = input.trim().parse::<usize>()?;
            out_ports
                .get(selection)
                .cloned()
                .ok_or_else(|| "invalid output port selected".into())
        }
    }
}

fn play_test_pattern(conn_out: &mut MidiOutputConnection) {
    const NOTE_ON: u8 = 0x90;
    const NOTE_OFF: u8 = 0x80;
    const VELOCITY: u8 = 0x64;
    const ROOT: u8 = 60; // Middle C

    let send = |conn: &mut MidiOutputConnection, msg: [u8; 3]| {
        let _ = conn.send(&msg);
    };

    let notes = [ROOT, ROOT + 4, ROOT + 7, ROOT + 12];

    println!("Sending a C major arpeggio over Bluetooth MIDI …");
    for note in notes {
        send(conn_out, [NOTE_ON, note, VELOCITY]);
        sleep(Duration::from_millis(200));
        send(conn_out, [NOTE_OFF, note, VELOCITY]);
    }
}
