//! Implementation of TaskContext for stackless coroutines

use alloc::boxed::Box;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

/// 任务状态
#[derive(Copy, Clone, PartialEq, Debug)]
#[allow(missing_docs)]
pub enum TaskStatus {
    Ready,
    Running,
    Blocked,
    Zombie,
}

/// 任务上下文
pub struct TaskContext {
    /// 任务的 Future（堆上分配的状态机）
    future: Pin<Box<dyn Future<Output = i32> + Send>>,

    status: TaskStatus,

    waker: Option<Waker>,
}

impl TaskContext {
    /// 从 Future 创建新的任务上下文
    pub fn new<F>(future: F) -> Self
    where
        F: Future<Output = i32> + Send + 'static,
    {
        Self {
            future: Box::pin(future),
            status: TaskStatus::Ready,
            waker: None,
        }
    }

    /// 轮询任务
    pub fn poll(&mut self, cx: &mut Context) -> Poll<i32> {
        self.future.as_mut().poll(cx)
    }

    /// 获取任务状态
    pub fn status(&self) -> TaskStatus {
        self.status
    }

    /// 设置任务状态
    pub fn set_status(&mut self, status: TaskStatus) {
        self.status = status;
    }

    /// 设置唤醒器
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    /// 获取唤醒器
    pub fn waker(&self) -> Option<Waker> {
        self.waker.clone()
    }
}
