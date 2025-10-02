// MoQ service (bridge to JS implementation)
// =====================================================

struct JsMoqService {
    handle: Rc<RefCell<Option<JsValue>>>,
    listener: Rc<RefCell<Option<Box<dyn MoqListener>>>>,
    pending: Rc<RefCell<Vec<Vec<u8>>>>,
    ready: Rc<RefCell<bool>>,
    on_ready: Rc<RefCell<Option<Closure<dyn FnMut()>>>>,
    on_frame: Rc<RefCell<Option<Closure<dyn FnMut(Uint8Array)>>>>,
    on_error: Rc<RefCell<Option<Closure<dyn FnMut(JsValue)>>>>,
    on_closed: Rc<RefCell<Option<Closure<dyn FnMut()>>>>,
}

impl JsMoqService {
    fn new() -> Self {
        Self {
            handle: Rc::new(RefCell::new(None)),
            listener: Rc::new(RefCell::new(None)),
            pending: Rc::new(RefCell::new(Vec::new())),
            ready: Rc::new(RefCell::new(false)),
            on_ready: Rc::new(RefCell::new(None)),
            on_frame: Rc::new(RefCell::new(None)),
            on_error: Rc::new(RefCell::new(None)),
            on_closed: Rc::new(RefCell::new(None)),
        }
    }

    fn flush_pending(&self) {
        if !*self.ready.borrow() {
            return;
        }
        let handle = match self.handle.borrow().as_ref() {
            Some(handle) => handle.clone(),
            None => return,
        };
        let publish = match get_bridge_method(&handle, "publish") {
            Ok(fun) => fun,
            Err(err) => {
                log::error!("missing moq publish: {:?}", err);
                return;
            }
        };
        while let Some(bytes) = self.pending.borrow_mut().pop() {
            let buffer = Uint8Array::from(bytes.as_slice());
            if let Err(err) = publish.call1(&handle, &buffer.into()) {
                log::error!("moq publish failed: {:?}", err);
            }
        }
    }
}

impl MoqService for JsMoqService {
    fn connect(
        &self,
        url: &str,
        session: &str,
        role: SessionRole,
        peer_role: SessionRole,
        listener: Box<dyn MoqListener>,
    ) {
        *self.listener.borrow_mut() = Some(listener);
        let bridge = match get_moq_bridge() {
            Ok(obj) => obj,
            Err(err) => {
                log::error!("moq bridge missing: {:?}", err);
                return;
            }
        };
        let connect = match get_bridge_method(&bridge, "connect") {
            Ok(fun) => fun,
            Err(err) => {
                log::error!("moq connect missing: {:?}", err);
                return;
            }
        };

        let params = json!({
            "relay": url,
            "session": session,
            "role": role.as_str(),
            "peerRole": peer_role.as_str(),
        });
        let params_js = swb::to_value(&params).unwrap_or(JsValue::NULL);

        let ready_flag = self.ready.clone();
        let handle_cell = self.handle.clone();
        let listener_cell = self.listener.clone();
        let listener_for_ready = self.listener.clone();
        let on_ready_closure = Closure::wrap(Box::new(move || {
            *ready_flag.borrow_mut() = true;
            if let Some(listener) = listener_for_ready.borrow().as_ref() {
                listener.on_ready();
            }
        }) as Box<dyn FnMut()>);

        let listener_for_frame = listener_cell.clone();
        let on_frame_closure = Closure::wrap(Box::new(move |buffer: Uint8Array| {
            let mut data = vec![0u8; buffer.length() as usize];
            buffer.copy_to(&mut data[..]);
            if let Some(listener) = listener_for_frame.borrow().as_ref() {
                listener.on_frame(data);
            }
        }) as Box<dyn FnMut(Uint8Array)>);

        let listener_for_error = listener_cell.clone();
        let on_error_closure = Closure::wrap(Box::new(move |value: JsValue| {
            let message = value
                .as_string()
                .unwrap_or_else(|| String::from("unknown error"));
            if let Some(listener) = listener_for_error.borrow().as_ref() {
                listener.on_error(message);
            }
        }) as Box<dyn FnMut(JsValue)>);

        let listener_for_closed = listener_cell.clone();
        let on_closed_closure = Closure::wrap(Box::new(move || {
            if let Some(listener) = listener_for_closed.borrow().as_ref() {
                listener.on_closed();
            }
        }) as Box<dyn FnMut()>);

        let callbacks_obj = Object::new();
        let _ = Reflect::set(
            &callbacks_obj,
            &JsValue::from_str("onReady"),
            on_ready_closure.as_ref(),
        );
        let _ = Reflect::set(
            &callbacks_obj,
            &JsValue::from_str("onFrame"),
            on_frame_closure.as_ref(),
        );
        let _ = Reflect::set(
            &callbacks_obj,
            &JsValue::from_str("onError"),
            on_error_closure.as_ref(),
        );
        let _ = Reflect::set(
            &callbacks_obj,
            &JsValue::from_str("onClosed"),
            on_closed_closure.as_ref(),
        );

        *self.on_ready.borrow_mut() = Some(on_ready_closure);
        *self.on_frame.borrow_mut() = Some(on_frame_closure);
        *self.on_error.borrow_mut() = Some(on_error_closure);
        *self.on_closed.borrow_mut() = Some(on_closed_closure);

        let promise = match connect.call2(&bridge, &params_js, &callbacks_obj.into()) {
            Ok(value) => value,
            Err(err) => {
                log::error!("moq connect call failed: {:?}", err);
                return;
            }
        };

        let promise: js_sys::Promise = match promise.dyn_into() {
            Ok(p) => p,
            Err(err) => {
                log::error!("moq connect did not return a Promise: {:?}", err);
                return;
            }
        };

        let handle_cell_clone = handle_cell.clone();
        let service_clone = self.clone();
        spawn_local(async move {
            match JsFuture::from(promise).await {
                Ok(handle) => {
                    *handle_cell_clone.borrow_mut() = Some(handle.clone());
                    *service_clone.ready.borrow_mut() = true;
                    service_clone.flush_pending();
                }
                Err(err) => {
                    log::error!("moq connect promise rejected: {:?}", err);
                }
            }
        });
    }

    fn publish_wrapper(&self, bytes: &[u8]) {
        if !*self.ready.borrow() {
            self.pending.borrow_mut().push(bytes.to_vec());
            return;
        }
        let handle = match self.handle.borrow().as_ref() {
            Some(handle) => handle.clone(),
            None => {
                self.pending.borrow_mut().push(bytes.to_vec());
                return;
            }
        };
        let publish = match get_bridge_method(&handle, "publish") {
            Ok(fun) => fun,
            Err(err) => {
                log::error!("moq publish missing: {:?}", err);
                return;
            }
        };
        let buffer = Uint8Array::from(bytes);
        if let Err(err) = publish.call1(&handle, &buffer.into()) {
            log::error!("moq publish error: {:?}", err);
        }
    }

    fn shutdown(&self) {
        if let Some(handle) = self.handle.borrow_mut().take() {
            if let Ok(close) = get_bridge_method(&handle, "close") {
                let _ = close.call0(&handle);
            }
        }
        *self.ready.borrow_mut() = false;
        self.pending.borrow_mut().clear();
        self.listener.borrow_mut().take();
        self.on_ready.borrow_mut().take();
        self.on_frame.borrow_mut().take();
        self.on_error.borrow_mut().take();
        self.on_closed.borrow_mut().take();
    }
}

impl Clone for JsMoqService {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            listener: self.listener.clone(),
            pending: self.pending.clone(),
            ready: self.ready.clone(),
            on_ready: self.on_ready.clone(),
            on_frame: self.on_frame.clone(),
            on_error: self.on_error.clone(),
            on_closed: self.on_closed.clone(),
        }
    }
}

