use named_pipe::{
    ConnectingServer,
    PipeClient as _PipeClient,
    PipeOptions,
    PipeServer as _PipeServer
};
use std::convert::TryFrom;
use std::io;
use bufstream::BufStream;
pub use named_pipe::OpenMode;

use serde::{Serialize};
use serde::de::DeserializeOwned;
use serde_json;
// trait requires being in direct scope for read/write methods
use std::io::{Write, BufRead, Error, ErrorKind};


pub struct PipeServer {
    buffer: Option<BufStream<_PipeServer>>,
    pipe_options: PipeOptions,
    connecting_server: Option<ConnectingServer>,
    started_server: bool,
    out_buffer: usize,
    in_buffer: usize
}

impl PipeServer {
    // OpenMode defaults to duplex, but can be changed
    pub fn new<S: AsRef<str>>(name: S) -> PipeServer
    {
        let full_name = name.as_ref().into();
        let pipe_name = check_pipe_name_syntax(full_name);

        let options = PipeOptions::new(pipe_name);

        PipeServer {
            buffer: None,
            pipe_options: options,
            connecting_server: None,
            started_server: false,
            out_buffer: 65536,
            in_buffer: 65536
        }
    }
    
    // CONFIGURATION OPTIONS
    pub fn open_mode(mut self, mode: OpenMode) -> PipeServer {
        self.pipe_options.open_mode(mode);
        self
    }

    pub fn first(mut self, val: bool) -> PipeServer {
        self.pipe_options.first(val);
        self
    }

    pub fn in_buffer(mut self, val: u32) -> PipeServer {
        self.pipe_options.in_buffer(val);
        self.in_buffer = usize::try_from(val).unwrap();
        self
    }

    pub fn out_buffer(mut self, val: u32) -> PipeServer {
        self.pipe_options.out_buffer(val);
        self.out_buffer = usize::try_from(val).unwrap();
        self
    }

    // Makes an identical clone to this one
    // If current server was already started
    // This instance will also be started
    // HOWEVER: it will NOT create a new buffer
    // BECAUSE you need to wait() for a new client
    pub fn clone(&self) -> PipeServer {
        let mut newserver = PipeServer {
            buffer: None,
            pipe_options: self.pipe_options.clone(),
            connecting_server: None,
            started_server: false,
            ..*self
        };

        if self.connecting_server.is_some() {
            newserver.start().unwrap();
        }

        newserver
    }

    pub fn start(&mut self) -> io::Result<()> {
        if !self.started_server {
            self.connecting_server = match self.pipe_options.single() {
                Ok(c) => Some(c),
                _ => return Err(Error::new(ErrorKind::Other, "Failed to start server"))
            };

            self.started_server = true;
            Ok(())
        } else {
            Err(Error::new(ErrorKind::AlreadyExists, "Server already exists. Use clone() to make new one"))
        }
    }


    // This will return multiple new instances.
    // It will move self to the resulting Vec
    pub fn start_multiple(mut self, num: u32) -> io::Result<Vec<PipeServer>> {
        if self.started_server {
            return Err(Error::new(ErrorKind::AlreadyExists, "Server already exists. Use clone() to make new one"));
        }

        // 1 less because we're also putting this server into it at the end
        let servers = match self.pipe_options.multiple(num-1) {
            Ok(c) => c,
            _ => return Err(Error::new(ErrorKind::Other, "Failed to start server"))
        };

        let mut pipeservers: Vec<PipeServer> = Vec::new();

        // copy all data to new pipeserver, along with connecting server
        for c in servers {
            let server = PipeServer {
                buffer: None,
                pipe_options: self.pipe_options.clone(),
                connecting_server: Some(c),
                started_server: true,
                ..self
            };

            pipeservers.push(server);
        }

        // finally move this server to the list
        self.start().unwrap();
        pipeservers.push(self);

        Ok(pipeservers)
    }

    /// This function will flush buffers and disconnect server from client. Then will start waiting
    /// for a new client.
    pub fn disconnect(&mut self) -> io::Result<()> {
        // meh, rip it out of hte buffer, but whatever..
        let connserver = self.buffer.take().unwrap().into_inner().unwrap().disconnect().unwrap();

        self.connecting_server = Some(connserver);
        Ok(())
    }

    pub fn wait(&mut self) -> io::Result<()> {
        if self.connecting_server.is_none() {
            return Err(Error::new(ErrorKind::NotConnected,"Did you start() it yet?"));
        }

        let pipe_server = self.connecting_server.take().unwrap().wait().unwrap();
        self.buffer = Some(BufStream::new(pipe_server));

        Ok(())
    }

    pub fn wait_ms(&mut self, timeout: u32) -> io::Result<()> {
        if self.connecting_server.is_none() {
            return Err(Error::new(ErrorKind::NotConnected,"Did you start() it yet?"));
        }

        let pipe_server = self.connecting_server.take().unwrap().wait_ms(timeout).unwrap().unwrap();
        self.buffer = Some(BufStream::new(pipe_server));

        Ok(())
    }

    pub fn read<T>(&mut self) -> io::Result<Option<T>>
        where T: DeserializeOwned
    {
        if self.buffer.is_none() {
            return Err(Error::new(ErrorKind::NotFound, "Need to start() the server and wait()"));
        }

        // take ownership cause we need it for the buffer write
        let mut stream = self.buffer.take().unwrap();

        let mut buf = String::new();
        let n = stream.read_line(&mut buf)?;

        // this will probably never trigger, cause input buffer would've already limited it
        if buf.len() > self.in_buffer {
            return Err(io::Error::new(ErrorKind::InvalidData, "Read buffer size exceeded limits"));
        }

        if n > 0 {
            let data = serde_json::from_str(&mut buf)?;

            // give ownership back to server
            self.buffer = Some(stream);
            Ok(Some(data))
        } else {
            self.buffer = Some(stream);
            Ok(None)
        }
    }

    pub fn write<T: ?Sized>(&mut self, data: &T) -> io::Result<()>
        where T: Serialize
    {
        if self.buffer.is_none() {
            return Err(Error::new(ErrorKind::NotFound, "Need to start() the server and wait()"));
        }

        // take ownership cause we need it for the buffer write
        let mut stream = self.buffer.take().unwrap();

        let mut buf = serde_json::to_string(data)?;
        buf.push('\n');

        if buf.len() > self.out_buffer {
            return Err(io::Error::new(ErrorKind::InvalidData, "Write buffer size exceeded limits"));
        }

        stream.write_all(buf.as_bytes())?;
        stream.flush()?;


        // give ownership back
        self.buffer = Some(stream);
        Ok(())
    }
}


pub struct PipeClient {
    name: String,
    buffer: Option<BufStream<_PipeClient>>,
    connected: bool
}

impl PipeClient {
    pub fn new<S: AsRef<str>>(name: S) -> PipeClient {
        let full_name = name.as_ref().into();
        let pipe_name = check_pipe_name_syntax(full_name);

        PipeClient {
            name: pipe_name,
            buffer: None,
            connected: false
        }
    }

    pub fn connect(&mut self) -> io::Result<()> {
        let client = match _PipeClient::connect(&self.name) {
            Ok(c) => c,
            Err(e) => return Err(e)
        };

        self.buffer = Some(BufStream::new(client));
        self.connected = true;

        Ok(())
    }

    pub fn connect_ms(&mut self, timeout: u32) -> io::Result<()> {
        let client = match _PipeClient::connect_ms(&self.name, timeout) {
            Ok(c) => c,
            Err(e) => return Err(e)
        };

        self.buffer = Some(BufStream::new(client));
        self.connected = true;

        Ok(())
    }

    pub fn read<T>(&mut self) -> io::Result<Option<T>>
        where T: DeserializeOwned
    {
        if !self.connected {
            return Err(Error::new(ErrorKind::NotFound, "Need to connect() to the server first"));
        }

        // take ownership cause we need it for the buffer write
        let mut stream = self.buffer.take().unwrap();

        let mut buf = String::new();
        let n = stream.read_line(&mut buf)?;
        if n > 0 {
            let data = serde_json::from_str(&mut buf)?;

            // give ownership back to server
            self.buffer = Some(stream);
            Ok(Some(data))
        } else {
            self.buffer = Some(stream);
            Ok(None)
        }
    }

    pub fn write<T: ?Sized>(&mut self, data: &T) -> io::Result<()>
        where T: Serialize
    {
        if !self.connected {
            return Err(Error::new(ErrorKind::NotFound, "Need to connect() to the server first"));
        }

        // take ownership cause we need it for the buffer write
        let mut stream = self.buffer.take().unwrap();

        let mut buf = serde_json::to_string(data)?;
        buf.push('\n');
        
        stream.write_all(buf.as_bytes())?;
        stream.flush()?;


        // give ownership back
        self.buffer = Some(stream);
        Ok(())
    }
}


// Check syntax of pipe name to ensure it has proper directory structure
fn check_pipe_name_syntax(name: &str) -> String {
    let mut pipe_name: String = String::from(name);
    let pipe_syntax = r"\\.\pipe\";

    // make proper syntax part of the path
    if !pipe_name.starts_with(pipe_syntax) {
        pipe_name.insert_str(0, pipe_syntax);
    }

    pipe_name
}