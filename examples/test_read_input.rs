use std::error::Error;
use std::io::{stdin, stdout, Write};
use ws2818_rgb_led_spi_driver::adapter_gen::{WS28xxAdapter, HardwareDev};
use ws2818_rgb_led_spi_driver::adapter_spi::WS28xxSpiAdapter;

use midir::{Ignore, MidiInput};

fn main() {
    match run() {
        Ok(_) => (),
        Err(err) => println!("Error: {}", err),
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut input = String::new();

    let mut midi_in = MidiInput::new("midir reading input")?;
    midi_in.ignore(Ignore::None);

    // Get an input port (read from console if multiple are available)
    let in_ports = midi_in.ports();
    let in_port = match in_ports.len() {
        0 => return Err("no input port found".into()),
        1 => {
            println!(
                "Choosing the only available input port: {}",
                midi_in.port_name(&in_ports[0]).unwrap()
            );
            &in_ports[0]
        }
        _ => {
            println!("\nAvailable input ports:");
            for (i, p) in in_ports.iter().enumerate() {
                println!("{}: {}", i, midi_in.port_name(p).unwrap());
            }
            print!("Please select input port: ");
            stdout().flush()?;
            let mut input = String::new();
            stdin().read_line(&mut input)?;
            in_ports
                .get(input.trim().parse::<usize>()?)
                .ok_or("invalid input port selected")?
        }
    };

    println!("\nOpening connection");
    let in_port_name = midi_in.port_name(in_port)?;

    let (num_leds, r, g, b) = (176, 0, 0, 0);
    let mut data = vec![(r, g, b); num_leds];
    let mut led_offset = 0;

    // _conn_in needs to be a named parameter, because it needs to be kept alive until the end of the scope
    let _conn_in = midi_in.connect(
        in_port,
        "midir-read-input",
        move |stamp, message, _| {
            if message[0] != 254 {
                println!("{}: {:?} (len = {})", stamp, message, message.len());

                let mut adapter = WS28xxSpiAdapter::new("/dev/spidev0.0").unwrap();

                if message[1] < 56 {
                    led_offset = 39;
                } else if message[1] < 69 {
                    led_offset = 40;
                } else if message[1] < 93 {
                    led_offset = 41;
                } else {
                    led_offset = 42;
                }

                match message[0] {
                    144 => { // Note on
                        // data[message[1] as usize * 2 - 39] = (0, 0, message[2] as u8);
                        data[message[1] as usize * 2 - led_offset] = (0, 0, 1);
                        adapter.write_rgb(&data).unwrap();
                    }
                    128 => { // Note off
                        data[message[1] as usize * 2 - led_offset] = (0, 0, 0);
                        adapter.write_rgb(&data).unwrap();
                    }
                    _ => (),
                }

                adapter.write_rgb(&data).unwrap();
            }
        },
        (),
    )?;

    println!(
        "Connection open, reading input from '{}' (press enter to exit) ...",
        in_port_name
    );

    input.clear();
    stdin().read_line(&mut input)?; // wait for next enter key press

    println!("Closing connection");
    Ok(())
}
