use super::instance::*;
use super::module::*;
use super::register::*;
use super::signal::*;

use typed_arena::Arena;

use std::cell::RefCell;
use std::collections::BTreeMap;

/// A top-level container/owner object for a module graph.
///
/// A `Context` owns all parts of a module graph, and provides an API for creating [`Module`] objects.
///
/// # Examples
///
/// ```
/// use kaze::*;
///
/// let c = Context::new();
/// let m = c.module("my_module");
/// m.output("out", m.input("in", 1));
/// ```
///
/// [`Module`]: ./struct.Module.html
#[must_use]
pub struct Context<'a> {
    module_arena: Arena<Module<'a>>,
    pub(super) signal_arena: Arena<Signal<'a>>,
    pub(super) register_data_arena: Arena<RegisterData<'a>>,
    pub(super) register_arena: Arena<Register<'a>>,
    pub(super) instance_arena: Arena<Instance<'a>>,

    pub(super) modules: RefCell<BTreeMap<String, &'a Module<'a>>>,
}

impl<'a> Context<'a> {
    /// Creates a new, empty `Context`.
    ///
    /// # Examples
    ///
    /// ```
    /// use kaze::*;
    ///
    /// let c = Context::new();
    /// ```
    pub fn new() -> Context<'a> {
        Context {
            module_arena: Arena::new(),
            signal_arena: Arena::new(),
            register_data_arena: Arena::new(),
            register_arena: Arena::new(),
            instance_arena: Arena::new(),

            modules: RefCell::new(BTreeMap::new()),
        }
    }

    /// Creates a new [`Module`] called `name` in this `Context`.
    ///
    /// Conventionally, `name` should be `snake_case`, though this is not enforced.
    ///
    /// # Panics
    ///
    /// Panics if a [`Module`] with the same `name` already exists in this `Context`.
    ///
    /// # Examples
    ///
    /// ```
    /// use kaze::*;
    ///
    /// let c = Context::new();
    ///
    /// let my_module = c.module("my_module");
    /// let another_mod = c.module("another_mod");
    /// ```
    ///
    /// The following example panics by creating a `Module` with the same `name` as a previously-created `Module` in the same `Context`:
    ///
    /// ```should_panic
    /// use kaze::*;
    ///
    /// let c = Context::new();
    ///
    /// let _ = c.module("a"); // Unique name, OK
    /// let _ = c.module("b"); // Unique name, OK
    ///
    /// let _ = c.module("a"); // Non-unique name, panic!
    /// ```
    ///
    /// [`Module`]: ./struct.Module.html
    pub fn module<S: Into<String>>(&'a self, name: S) -> &Module {
        let name = name.into();
        let mut modules = self.modules.borrow_mut();
        if modules.contains_key(&name) {
            panic!(
                "A module with the name \"{}\" already exists in this context.",
                name
            );
        }
        let module = self.module_arena.alloc(Module::new(self, name.clone()));
        modules.insert(name, module);
        module
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "A module with the name \"a\" already exists in this context.")]
    fn unique_module_names() {
        let c = Context::new();

        let _ = c.module("a"); // Unique name, OK
        let _ = c.module("b"); // Unique name, OK

        // Panic
        let _ = c.module("a");
    }
}
