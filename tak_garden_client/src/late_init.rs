use once_cell::unsync::OnceCell;

// An owned value that cannot be set immediately, e.g. due to having to set up circular references, but will be initialised exactly once to hold a value.
// Accessing the inner value before it is initialised will panic.
// Any code constructing a LateInit value directly or indirectly should ensure that it gets initialised
// before it is handed to external code, to ensure that external code can safely assume there is an internal value to be interacted with.
#[derive(Debug)]
pub struct LateInit<T>(OnceCell<T>);

impl<T> LateInit<T> {
  pub fn init(&self, value: T) {
    assert!(self.0.set(value).is_ok())
  }
}

impl<T> Default for LateInit<T> {
  fn default() -> Self {
    LateInit(OnceCell::default())
  }
}

impl<T> std::ops::Deref for LateInit<T> {
  type Target = T;
  fn deref(&self) -> &Self::Target {
    self.0.get().unwrap()
  }
}

impl<T> std::ops::DerefMut for LateInit<T> {
  fn deref_mut(&mut self) -> &mut Self::Target {
    self.0.get_mut().unwrap()
  }
}
