use super::*;

pub struct Host {
    members: Arc<Members>,
    pairing: Mutex<Option<Live>>,
}

#[allow(clippy::type_complexity)]
struct Members {
    me: Hello,
    notebook: String,
    reach: Option<Reach>,
    relay: Option<String>,
    sharing: Mutex<Sharing>,
    changes: Mutex<()>,
    pending: Mutex<BTreeMap<[u8; 16], (Arc<Hello>, Line)>>,
    access: Mutex<HashMap<[u8; 16], Live>>,
    room: Mutex<Option<Live>>,
    served: Arc<Served>,
    events: Arc<dyn Fn() + Send + Sync>,
    keep: Box<dyn Fn(&Sharing) -> io::Result<()> + Send + Sync>,
}

impl Host {
    /// Shares a notebook, keeping every credential change before granting or revoking access.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        storage: Box<dyn Storage>,
        me: Hello,
        sharing: Sharing,
        notebook: &str,
        reach: Option<Reach>,
        relay: Option<&str>,
        events: impl Fn() + Send + Sync + 'static,
        keep: impl Fn(&Sharing) -> io::Result<()> + Send + Sync + 'static,
    ) -> io::Result<Self> {
        keep(&sharing)?;
        let members = Arc::new(Members {
            me,
            notebook: notebook.to_owned(),
            reach,
            relay: relay.map(str::to_owned),
            sharing: Mutex::new(sharing),
            changes: Mutex::default(),
            pending: Mutex::default(),
            access: Mutex::default(),
            room: Mutex::default(),
            served: Arc::new(Served {
                storage,
                images: Mutex::default(),
                snapshots: Mutex::default(),
                puts: Mutex::default(),
                guests: Mutex::default(),
                writers: Mutex::default(),
                host: Mutex::default(),
                room: Mutex::default(),
            }),
            events: Arc::new(events),
            keep: Box::new(keep),
        });
        let sharing = members.sharing.lock().unwrap().clone();
        members.presence(sharing.secret)?;
        for device in &sharing.members {
            members.open(device.secret)?;
        }
        let pairing = Mutex::new(Some(members.pair(&sharing)?));
        Ok(Self { members, pairing })
    }

    pub fn code(&self) -> Option<String> {
        let mut pairing = self.pairing.lock().unwrap();
        let pairing = pairing.as_mut()?;
        let mut sharing = self.members.sharing.lock().unwrap();
        if pairing.burned() {
            let mut next = sharing.clone();
            next.code = super::super::code::secret().ok()?;
            (self.members.keep)(&next).ok()?;
            *pairing = self.members.pair(&next).ok()?;
            *sharing = next;
        }
        let code = pairing.code();
        if let Some(code) = &code
            && *code != sharing.code
        {
            let mut next = sharing.clone();
            next.code = code.clone();
            (self.members.keep)(&next).ok()?;
            *sharing = next;
        }
        code
    }

    pub fn sharing(&self) -> Sharing {
        self.code();
        self.members.sharing.lock().unwrap().clone()
    }

    pub fn relayed(&self) -> Relayed {
        self.pairing
            .lock()
            .unwrap()
            .as_ref()
            .map_or(Relayed::Unknown, Live::relayed)
    }

    pub fn guests(&self) -> Vec<Peer> {
        self.members
            .room
            .lock()
            .unwrap()
            .as_ref()
            .map(Live::peers)
            .unwrap_or_default()
    }

    pub fn devices(&self) -> Vec<(Device, bool)> {
        let sharing = self.members.sharing.lock().unwrap();
        let access = self.members.access.lock().unwrap();
        sharing
            .members
            .iter()
            .map(|device| {
                let connected = access
                    .get(&device.secret)
                    .is_some_and(|live| !live.peers().is_empty());
                (device.clone(), connected)
            })
            .collect()
    }

    pub fn requests(&self) -> Vec<Arc<Hello>> {
        self.members
            .pending
            .lock()
            .unwrap()
            .values()
            .map(|(hello, _)| Arc::clone(hello))
            .collect()
    }

    pub fn approve(&self, approve: bool) -> io::Result<()> {
        let mut sharing = self.members.sharing.lock().unwrap();
        let mut next = sharing.clone();
        next.approve = approve;
        (self.members.keep)(&next)?;
        *sharing = next;
        drop(sharing);
        if !approve {
            for hello in self.requests() {
                self.allow(&hello.peer)?;
            }
        }
        (self.members.events)();
        Ok(())
    }

    pub fn allow(&self, peer: &[u8; 16]) -> io::Result<()> {
        let pending = self.members.pending.lock().unwrap().remove(peer);
        if let Some((hello, line)) = pending {
            if let Err(error) = self.members.grant(&hello, &line) {
                self.members
                    .pending
                    .lock()
                    .unwrap()
                    .insert(*peer, (hello, line));
                return Err(error);
            }
            (self.members.events)();
        }
        Ok(())
    }

    pub fn decline(&self, peer: &[u8; 16]) {
        if let Some((_, line)) = self.members.pending.lock().unwrap().remove(peer) {
            let _ = line.send(kind::APPROVAL, &wire::Approval::Declined);
            (self.members.events)();
        }
    }

    pub fn remove(&self, secret: &[u8; 16]) -> io::Result<()> {
        let _change = self.members.changes.lock().unwrap();
        let mut sharing = self.members.sharing.lock().unwrap();
        if !sharing
            .members
            .iter()
            .any(|device| device.secret == *secret)
        {
            return Ok(());
        }
        let mut next = sharing.clone();
        next.members.retain(|device| device.secret != *secret);
        getrandom::fill(&mut next.secret)
            .map_err(|_| io::Error::other("System random source failed"))?;
        next.code = super::super::code::secret()?;
        (self.members.keep)(&next)?;
        *sharing = next.clone();
        drop(sharing);
        let removed = self.members.access.lock().unwrap().remove(secret);
        if let Some(removed) = removed {
            for peer in removed.peers() {
                self.members.served.forget(&peer.hello.peer);
            }
            removed.leave("removed");
        }
        self.members.presence(next.secret)?;
        *self.pairing.lock().unwrap() = Some(self.members.pair(&next)?);
        let access = self.members.access.lock().unwrap();
        for device in &next.members {
            if let Some(live) = access.get(&device.secret) {
                live.sender().send(
                    kind::WELCOME,
                    &self.members.welcome(&next, device.secret),
                    None,
                );
            }
        }
        (self.members.events)();
        Ok(())
    }

    pub fn set_presence(&self, presence: Presence) {
        if let Some(room) = &*self.members.room.lock().unwrap() {
            room.set_presence(presence);
        }
    }

    pub fn on_changed(&self, listener: crate::session::Listener) {
        *self.members.served.host.lock().unwrap() = Some(listener);
    }

    pub fn touched(&self, paths: &[String]) {
        for path in paths {
            self.members.served.changed_here(path);
        }
        self.members.served.tell(paths);
    }

    pub fn stop(&self) {
        let _change = self.members.changes.lock().unwrap();
        drop(self.pairing.lock().unwrap().take());
        self.members.pending.lock().unwrap().clear();
        let access = std::mem::take(&mut *self.members.access.lock().unwrap());
        for (_, live) in access {
            live.leave(STOPPED);
        }
        drop(self.members.room.lock().unwrap().take());
    }
}

impl Members {
    fn welcome(&self, sharing: &Sharing, secret: [u8; 16]) -> Welcome {
        Welcome {
            share: sharing.share,
            secret,
            room: sharing.secret,
            notebook: self.notebook.clone(),
            host: self.me.name.clone(),
        }
    }

    fn presence(self: &Arc<Self>, secret: [u8; 16]) -> io::Result<()> {
        let told = Arc::clone(&self.events);
        let live = Live::start(
            self.me.clone(),
            &Room::Notebook(secret),
            self.reach,
            self.relay.as_deref(),
            move |_| told(),
        )?;
        *self.served.room.lock().unwrap() = Some(live.sender());
        *self.room.lock().unwrap() = Some(live);
        Ok(())
    }

    fn open(self: &Arc<Self>, secret: [u8; 16]) -> io::Result<()> {
        let members = Arc::downgrade(self);
        let live = Live::start(
            Hello {
                serves: Some(self.sharing.lock().unwrap().share),
                ..self.me.clone()
            },
            &Room::Notebook(secret),
            self.reach,
            self.relay.as_deref(),
            move |event| {
                let Some(members) = members.upgrade() else {
                    return;
                };
                match event {
                    Event::Met(hello, line) => {
                        let sharing = members.sharing.lock().unwrap();
                        if !sharing.members.iter().any(|device| device.secret == secret) {
                            line.hang_up("removed");
                            return;
                        }
                        members.served.admit(hello.peer, line);
                        let _ = line.send(kind::WELCOME, &members.welcome(&sharing, secret));
                    }
                    Event::Left(hello) => members.served.forget(&hello.peer),
                    Event::Frame { from, kind, body }
                        if wire::KNOWN.contains(&kind) && kind > 256 && kind != kind::REPLY =>
                    {
                        members.served.queue(&from.peer, kind, body)
                    }
                    Event::Changed => (members.events)(),
                    _ => {}
                }
            },
        )?;
        self.access.lock().unwrap().insert(secret, live);
        Ok(())
    }

    fn grant(self: &Arc<Self>, hello: &Hello, line: &Line) -> io::Result<()> {
        let _change = self.changes.lock().unwrap();
        if self.room.lock().unwrap().is_none() {
            return Err(io::ErrorKind::NotConnected.into());
        }
        let mut secret = [0; 16];
        getrandom::fill(&mut secret)
            .map_err(|_| io::Error::other("System random source failed"))?;
        if self.sharing.lock().unwrap().members.len() >= 64 {
            return Err(io::ErrorKind::ResourceBusy.into());
        }
        self.open(secret)?;
        let kept = (|| -> io::Result<Welcome> {
            let mut sharing = self.sharing.lock().unwrap();
            let mut next = sharing.clone();
            next.members.push(Device {
                secret,
                name: hello.name.clone(),
                device: hello.device.clone(),
            });
            (self.keep)(&next)?;
            *sharing = next;
            Ok(self.welcome(&sharing, secret))
        })();
        let welcome = match kept {
            Ok(welcome) => welcome,
            Err(error) => {
                drop(self.access.lock().unwrap().remove(&secret));
                return Err(error);
            }
        };
        line.send(kind::WELCOME, &welcome)?;
        (self.events)();
        Ok(())
    }

    fn pair(self: &Arc<Self>, sharing: &Sharing) -> io::Result<Live> {
        let members = Arc::downgrade(self);
        Live::start(
            Hello {
                serves: None,
                ..self.me.clone()
            },
            &Room::share(&sharing.code, &sharing.password),
            self.reach,
            self.relay.as_deref(),
            move |event| {
                let Some(members) = members.upgrade() else {
                    return;
                };
                match event {
                    Event::Met(hello, line) => {
                        if members.sharing.lock().unwrap().approve {
                            let mut pending = members.pending.lock().unwrap();
                            if pending.len() < 32 {
                                pending.insert(hello.peer, (Arc::clone(hello), line.clone()));
                                let _ = line.send(kind::APPROVAL, &wire::Approval::Pending);
                            } else {
                                let _ = line.send(kind::APPROVAL, &wire::Approval::Failed);
                            }
                            drop(pending);
                            (members.events)();
                        } else if let Err(error) = members.grant(hello, line) {
                            eprintln!("Live Share: could not admit a device: {error}");
                            let _ = line.send(kind::APPROVAL, &wire::Approval::Failed);
                        }
                    }
                    Event::Left(hello) => {
                        members.pending.lock().unwrap().remove(&hello.peer);
                        (members.events)();
                    }
                    Event::Changed => (members.events)(),
                    _ => {}
                }
            },
        )
    }
}
