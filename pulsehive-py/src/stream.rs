//! Async event stream bridge — wraps Rust's `Stream<Item = HiveEvent>` as
//! a Python async iterator consumable via `async for event in stream`.

use std::pin::Pin;
use std::sync::Arc;

use futures::StreamExt;
use pyo3::exceptions::PyStopAsyncIteration;
use pyo3::prelude::*;
use tokio::sync::Mutex;

use pulsehive_core::event::HiveEvent;

use crate::events::PyHiveEvent;

/// Async event stream — yields HiveEvent objects via ``async for``.
///
/// Obtained from ``await hive.deploy(agents, tasks)``.
/// Consumed via::
///
///     async for event in stream:
///         print(event.event_type, event.data)
#[pyclass(name = "EventStream")]
pub struct PyEventStream {
    pub(crate) stream: Arc<Mutex<Pin<Box<dyn futures::Stream<Item = HiveEvent> + Send>>>>,
}

#[pymethods]
impl PyEventStream {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let stream = Arc::clone(&self.stream);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut guard = stream.lock().await;
            match guard.next().await {
                Some(event) => Ok(PyHiveEvent::from(event)),
                None => Err(PyStopAsyncIteration::new_err(())),
            }
        })
    }

    fn __repr__(&self) -> String {
        "EventStream(active)".to_string()
    }
}

/// Register stream classes with the Python module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyEventStream>()?;
    Ok(())
}
