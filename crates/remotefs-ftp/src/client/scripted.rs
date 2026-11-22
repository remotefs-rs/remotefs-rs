//! Scripted FTP control-connection helpers for tests without Docker.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};

pub(crate) fn read_scripted_command(control: &mut BufReader<TcpStream>) -> String {
    let mut line = String::new();
    control.read_line(&mut line).unwrap();
    line
}

/// Reads one line, returning the byte count (0 means the peer closed).
#[cfg(feature = "tokio")]
pub(crate) fn read_scripted_command_opt(
    control: &mut BufReader<TcpStream>,
    line: &mut String,
) -> usize {
    control.read_line(line).unwrap()
}

pub(crate) fn write_scripted_reply(control: &mut BufReader<TcpStream>, response: &str) {
    control.get_mut().write_all(response.as_bytes()).unwrap();
}

pub(crate) fn authenticate_scripted_connection(control: &mut BufReader<TcpStream>) {
    write_scripted_reply(control, "220 ready\r\n");
    assert_eq!(read_scripted_command(control), "USER anonymous\r\n");
    write_scripted_reply(control, "331 password\r\n");
    assert_eq!(read_scripted_command(control), "PASS \r\n");
    write_scripted_reply(control, "230 logged in\r\n");
    assert_eq!(read_scripted_command(control), "TYPE I\r\n");
    write_scripted_reply(control, "200 binary\r\n");
}

/// Answers `PASV` + `LIST path` with `body` and a `226` reply.
pub(crate) fn complete_scripted_list(control: &mut BufReader<TcpStream>, path: &str, body: &[u8]) {
    let data_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let data_port = data_listener.local_addr().unwrap().port();
    assert_eq!(read_scripted_command(control), "PASV\r\n");
    write_scripted_reply(
        control,
        &format!(
            "227 passive (127,0,0,1,{},{})\r\n",
            data_port / 256,
            data_port % 256
        ),
    );
    assert_eq!(read_scripted_command(control), format!("LIST {path}\r\n"));
    write_scripted_reply(control, "150 opening data\r\n");
    let (mut data, _) = data_listener.accept().unwrap();
    data.write_all(body).unwrap();
    drop(data);
    write_scripted_reply(control, "226 listing complete\r\n");
}

/// Answers `PASV` + `LIST path` with a `550` refusal.
pub(crate) fn refuse_scripted_list(control: &mut BufReader<TcpStream>, path: &str) {
    let data_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let data_port = data_listener.local_addr().unwrap().port();
    assert_eq!(read_scripted_command(control), "PASV\r\n");
    write_scripted_reply(
        control,
        &format!(
            "227 passive (127,0,0,1,{},{})\r\n",
            data_port / 256,
            data_port % 256
        ),
    );
    assert_eq!(read_scripted_command(control), format!("LIST {path}\r\n"));
    let (data, _) = data_listener.accept().unwrap();
    write_scripted_reply(control, "550 listing denied\r\n");
    drop(data);
}
