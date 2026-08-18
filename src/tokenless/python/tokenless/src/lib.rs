//! Python bindings for the in-process Tokenless runtime.

use std::path::PathBuf;

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use tokenless_runtime::{
    Attribution, CompressOptions, CompressResult, RuntimeConfig, TokenlessRuntime as NativeRuntime,
};

create_exception!(_native, TokenlessError, PyException);

/// Python view of one structured compression result.
#[pyclass(name = "CompressionResult", frozen, get_all)]
struct PyCompressionResult {
    output: String,
    compressed_output: String,
    disposition: String,
    applied: bool,
    before_tokens: usize,
    after_tokens: usize,
    stash_writes: Option<usize>,
    stash_errors: Option<usize>,
    unrecoverable_truncations: Option<usize>,
    stash_size: Option<usize>,
}

impl From<CompressResult> for PyCompressionResult {
    fn from(result: CompressResult) -> Self {
        let disposition = result.disposition.as_str().to_string();
        let applied = result.applied();
        Self {
            output: result.output,
            compressed_output: result.compressed_output,
            disposition,
            applied,
            before_tokens: result.before_tokens,
            after_tokens: result.after_tokens,
            stash_writes: result.stash_writes,
            stash_errors: result.stash_errors,
            unrecoverable_truncations: result.unrecoverable_truncations,
            stash_size: result.stash_size,
        }
    }
}

/// Reusable in-process Tokenless runtime exposed to Python.
#[pyclass(name = "TokenlessRuntime")]
struct PyTokenlessRuntime {
    inner: NativeRuntime,
}

#[pymethods]
impl PyTokenlessRuntime {
    #[new]
    #[pyo3(signature = (
        data_dir=None,
        *,
        compression_enabled=true,
        stats_enabled=true,
        sls_enabled=false
    ))]
    fn new(
        data_dir: Option<PathBuf>,
        compression_enabled: bool,
        stats_enabled: bool,
        sls_enabled: bool,
    ) -> PyResult<Self> {
        let inner = NativeRuntime::new(RuntimeConfig {
            data_dir,
            stats_enabled,
            sls_enabled,
            compression_enabled,
        })
        .map_err(to_python_error)?;
        Ok(Self { inner })
    }

    #[pyo3(signature = (
        input,
        *,
        truncate_strings_at=None,
        truncate_arrays_at=None,
        max_depth=None,
        agent_id="python",
        session_id=None,
        tool_use_id=None,
        stash_enabled=true,
        require_reversible=true
    ))]
    #[allow(clippy::too_many_arguments)]
    fn compress_response(
        &self,
        py: Python<'_>,
        input: String,
        truncate_strings_at: Option<usize>,
        truncate_arrays_at: Option<usize>,
        max_depth: Option<usize>,
        agent_id: &str,
        session_id: Option<String>,
        tool_use_id: Option<String>,
        stash_enabled: bool,
        require_reversible: bool,
    ) -> PyResult<PyCompressionResult> {
        let options = CompressOptions {
            truncate_strings_at,
            truncate_arrays_at,
            max_depth,
            stash_enabled,
            require_reversible,
        };
        let attribution = Attribution {
            agent_id: agent_id.to_string(),
            session_id,
            tool_use_id,
        };
        py.allow_threads(|| {
            self.inner
                .compress_response(&input, &options, &attribution)
                .map(PyCompressionResult::from)
                .map_err(to_python_error)
        })
    }

    #[pyo3(signature = (
        input,
        *,
        agent_id="python",
        session_id=None,
        tool_use_id=None
    ))]
    fn compress_schema(
        &self,
        py: Python<'_>,
        input: String,
        agent_id: &str,
        session_id: Option<String>,
        tool_use_id: Option<String>,
    ) -> PyResult<PyCompressionResult> {
        let attribution = Attribution {
            agent_id: agent_id.to_string(),
            session_id,
            tool_use_id,
        };
        py.allow_threads(|| {
            self.inner
                .compress_schema(&input, &attribution)
                .map(PyCompressionResult::from)
                .map_err(to_python_error)
        })
    }

    #[pyo3(signature = (
        input,
        *,
        agent_id="python",
        session_id=None,
        tool_use_id=None
    ))]
    fn compress_toon(
        &self,
        py: Python<'_>,
        input: String,
        agent_id: &str,
        session_id: Option<String>,
        tool_use_id: Option<String>,
    ) -> PyResult<PyCompressionResult> {
        let attribution = Attribution {
            agent_id: agent_id.to_string(),
            session_id,
            tool_use_id,
        };
        py.allow_threads(|| {
            self.inner
                .compress_toon(&input, &attribution)
                .map(PyCompressionResult::from)
                .map_err(to_python_error)
        })
    }

    fn retrieve(&self, py: Python<'_>, hash_or_marker: String) -> PyResult<String> {
        py.allow_threads(|| {
            self.inner
                .retrieve(&hash_or_marker)
                .map_err(to_python_error)
        })
    }

    #[getter]
    fn data_dir(&self) -> String {
        self.inner.data_dir().to_string_lossy().into_owned()
    }

    #[getter]
    fn stash_available(&self) -> bool {
        self.inner.stash_available()
    }

    #[getter]
    fn stash_error(&self) -> Option<String> {
        self.inner.stash_error().map(str::to_string)
    }

    #[getter]
    fn stats_available(&self) -> bool {
        self.inner.stats_available()
    }

    #[getter]
    fn stats_error(&self) -> Option<String> {
        self.inner.stats_error().map(str::to_string)
    }
}

fn to_python_error(error: tokenless_runtime::RuntimeError) -> PyErr {
    TokenlessError::new_err(error.to_string())
}

/// Register the native Tokenless module.
#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyTokenlessRuntime>()?;
    module.add_class::<PyCompressionResult>()?;
    module.add("TokenlessError", module.py().get_type::<TokenlessError>())?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
