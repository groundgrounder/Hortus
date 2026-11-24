// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::poll_fn;
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

/// 跨后台线程与 GLib 主循环异步传递批次的无轮询通道。
pub struct NotifyQueue<T> {
    inner: Mutex<NotifyQueueInner<T>>,
}

struct NotifyQueueInner<T> {
    items: VecDeque<T>,
    closed: bool,
    waker: Option<Waker>,
}

impl<T> NotifyQueue<T> {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(NotifyQueueInner {
                items: VecDeque::new(),
                closed: false,
                waker: None,
            }),
        })
    }

    pub fn push(&self, item: T) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.closed {
            return;
        }
        inner.items.push_back(item);
        if let Some(waker) = inner.waker.take() {
            waker.wake();
        }
    }

    pub fn close(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.closed = true;
        if let Some(waker) = inner.waker.take() {
            waker.wake();
        }
    }

    pub async fn next(&self) -> Option<T> {
        poll_fn(|cx| {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(item) = inner.items.pop_front() {
                Poll::Ready(Some(item))
            } else if inner.closed {
                Poll::Ready(None)
            } else {
                inner.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }
}

/// 主线程串行命令队列：保证回调与跨模块指令依次执行，结构上杜绝 `RefCell` 重入。
pub struct CommandQueue<C> {
    items: RefCell<VecDeque<C>>,
    draining: Cell<bool>,
}

impl<C> Default for CommandQueue<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> CommandQueue<C> {
    pub fn new() -> Self {
        Self {
            items: RefCell::new(VecDeque::new()),
            draining: Cell::new(false),
        }
    }

    pub fn dispatch(&self, cmd: C, mut handler: impl FnMut(C)) {
        self.items.borrow_mut().push_back(cmd);
        if self.draining.get() {
            return;
        }
        self.draining.set(true);
        loop {
            let next = self.items.borrow_mut().pop_front();
            let Some(item) = next else {
                break;
            };
            handler(item);
        }
        self.draining.set(false);
    }

    pub fn clear(&self) {
        self.items.borrow_mut().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn command_queue_runs_nested_commands_serially_without_reentrancy() {
        let queue = Rc::new(CommandQueue::new());
        let log = Rc::new(RefCell::new(Vec::new()));
        let active_borrow = Rc::new(RefCell::new(0i32));

        let q_clone = Rc::clone(&queue);
        let log_clone = Rc::clone(&log);
        let borrow_clone = Rc::clone(&active_borrow);
        let handler = move |cmd: i32| {
            let mut guard = borrow_clone.borrow_mut();
            *guard += 1;
            log_clone.borrow_mut().push(cmd);
            drop(guard);
            if cmd == 1 {
                assert_eq!(*log_clone.borrow(), vec![1]);
                let q2 = Rc::clone(&q_clone);
                let l2 = Rc::clone(&log_clone);
                q2.dispatch(2, |inner| {
                    l2.borrow_mut().push(inner);
                });
                // 嵌套入队不得同步重入执行，必须在 cmd == 1 返回后才出队。
                assert_eq!(*log_clone.borrow(), vec![1]);
            }
        };

        queue.dispatch(1, handler);
        assert_eq!(*log.borrow(), vec![1, 2]);
        assert_eq!(*active_borrow.borrow(), 2);
    }
}
