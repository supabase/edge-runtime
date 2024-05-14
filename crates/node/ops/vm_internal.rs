// Copyright 2018-2024 the Deno authors. All rights reserved. MIT license.

use deno_core::error::type_error;
use deno_core::error::AnyError;
use deno_core::v8;
use deno_core::v8::MapFnTo;
use std::rc::Rc;

pub const PRIVATE_SYMBOL_NAME: v8::OneByteConst =
    v8::String::create_external_onebyte_const(b"node:contextify:context");

/// An unbounded script that can be run in a context.
#[derive(Debug)]
pub struct ContextifyScript {
    script: v8::Global<v8::UnboundScript>,
}

impl ContextifyScript {
    pub fn new(
        scope: &mut v8::HandleScope,
        source_str: v8::Local<v8::String>,
    ) -> Result<Self, AnyError> {
        let source = v8::script_compiler::Source::new(source_str, None);

        let unbound_script = v8::script_compiler::compile_unbound_script(
            scope,
            source,
            v8::script_compiler::CompileOptions::NoCompileOptions,
            v8::script_compiler::NoCacheReason::NoReason,
        )
        .ok_or_else(|| type_error("Failed to compile script"))?;
        let script = v8::Global::new(scope, unbound_script);
        Ok(Self { script })
    }

    // TODO(littledivy): Support `options`
    pub fn eval_machine<'s>(
        &self,
        scope: &mut v8::HandleScope<'s>,
        _context: v8::Local<v8::Context>,
    ) -> Option<v8::Local<'s, v8::Value>> {
        let tc_scope = &mut v8::TryCatch::new(scope);

        let unbound_script = v8::Local::new(tc_scope, self.script.clone());
        let script = unbound_script.bind_to_current_context(tc_scope);

        let result = script.run(tc_scope);

        if tc_scope.has_caught() {
            // If there was an exception thrown during script execution, re-throw it.
            if !tc_scope.has_terminated() {
                tc_scope.rethrow();
            }

            return None;
        }

        Some(result.unwrap())
    }
}

#[derive(Debug, Clone)]
pub struct ContextifyContext {
    context: v8::Global<v8::Context>,
    sandbox: v8::Global<v8::Object>,
}

impl ContextifyContext {
    pub fn attach(scope: &mut v8::HandleScope, sandbox_obj: v8::Local<v8::Object>) {
        let tmp = init_global_template(scope);

        let context = create_v8_context(scope, tmp, None);
        Self::from_context(scope, context, sandbox_obj);
    }

    fn from_context(
        scope: &mut v8::HandleScope,
        v8_context: v8::Local<v8::Context>,
        sandbox_obj: v8::Local<v8::Object>,
    ) {
        let main_context = scope.get_current_context();
        let context_state = main_context
            .get_slot::<Rc<deno_core::ContextState>>(scope)
            .unwrap()
            .clone();

        v8_context.set_security_token(main_context.get_security_token(scope));
        v8_context.set_slot(scope, context_state);

        let context = v8::Global::new(scope, v8_context);
        let sandbox = v8::Global::new(scope, sandbox_obj);
        let wrapper = deno_core::cppgc::make_cppgc_object(scope, Self { context, sandbox });
        let ptr = deno_core::cppgc::try_unwrap_cppgc_object::<Self>(wrapper.into()).unwrap();

        // SAFETY: We are storing a pointer to the ContextifyContext
        // in the embedder data of the v8::Context. The contextified wrapper
        // lives longer than the execution context, so this should be safe.
        unsafe {
            v8_context
                .set_aligned_pointer_in_embedder_data(1, ptr as *const ContextifyContext as _);
        }

        let private_str = v8::String::new_from_onebyte_const(scope, &PRIVATE_SYMBOL_NAME);
        let private_symbol = v8::Private::for_api(scope, private_str);

        sandbox_obj.set_private(scope, private_symbol, wrapper.into());
    }

    pub fn from_sandbox_obj<'a>(
        scope: &mut v8::HandleScope,
        sandbox_obj: v8::Local<v8::Object>,
    ) -> Option<&'a Self> {
        let private_str = v8::String::new_from_onebyte_const(scope, &PRIVATE_SYMBOL_NAME);
        let private_symbol = v8::Private::for_api(scope, private_str);

        sandbox_obj
            .get_private(scope, private_symbol)
            .and_then(|wrapper| deno_core::cppgc::try_unwrap_cppgc_object::<Self>(wrapper))
    }

    pub fn is_contextify_context(
        scope: &mut v8::HandleScope,
        object: v8::Local<v8::Object>,
    ) -> bool {
        Self::from_sandbox_obj(scope, object).is_some()
    }

    pub fn context<'a>(&self, scope: &mut v8::HandleScope<'a>) -> v8::Local<'a, v8::Context> {
        v8::Local::new(scope, &self.context)
    }

    fn global_proxy<'s>(&self, scope: &mut v8::HandleScope<'s>) -> v8::Local<'s, v8::Object> {
        let ctx = self.context(scope);
        ctx.global(scope)
    }

    fn sandbox<'a>(&self, scope: &mut v8::HandleScope<'a>) -> v8::Local<'a, v8::Object> {
        v8::Local::new(scope, &self.sandbox)
    }

    fn get<'a, 'c>(
        scope: &mut v8::HandleScope<'a>,
        object: v8::Local<'a, v8::Object>,
    ) -> Option<&'c ContextifyContext> {
        let context = object.get_creation_context(scope)?;

        let context_ptr = context.get_aligned_pointer_from_embedder_data(1);
        // SAFETY: We are storing a pointer to the ContextifyContext
        // in the embedder data of the v8::Context during creation.
        Some(unsafe { &*(context_ptr as *const ContextifyContext) })
    }
}

pub const VM_CONTEXT_INDEX: usize = 0;

fn create_v8_context<'a>(
    scope: &mut v8::HandleScope<'a>,
    object_template: v8::Local<v8::ObjectTemplate>,
    snapshot_data: Option<&'static [u8]>,
) -> v8::Local<'a, v8::Context> {
    let scope = &mut v8::EscapableHandleScope::new(scope);

    let context = if let Some(_snapshot_data) = snapshot_data {
        v8::Context::from_snapshot(scope, VM_CONTEXT_INDEX).unwrap()
    } else {
        v8::Context::new_from_template(scope, object_template)
    };

    scope.escape(context)
}

#[derive(Debug, Clone)]
struct SlotContextifyGlobalTemplate(v8::Global<v8::ObjectTemplate>);

fn init_global_template<'a>(scope: &mut v8::HandleScope<'a>) -> v8::Local<'a, v8::ObjectTemplate> {
    let mut maybe_object_template_slot = scope.get_slot::<SlotContextifyGlobalTemplate>();

    if maybe_object_template_slot.is_none() {
        init_global_template_inner(scope);
        maybe_object_template_slot = scope.get_slot::<SlotContextifyGlobalTemplate>();
    }
    let object_template_slot = maybe_object_template_slot
        .expect("ContextifyGlobalTemplate slot should be already populated.")
        .clone();
    v8::Local::new(scope, object_template_slot.0)
}

extern "C" fn c_noop(_: *const v8::FunctionCallbackInfo) {}

// Using thread_local! to get around compiler bug.
//
// See NOTE in ext/node/global.rs#L12
thread_local! {
  pub static GETTER_MAP_FN: v8::GenericNamedPropertyGetterCallback<'static> = property_getter.map_fn_to();
  pub static SETTER_MAP_FN: v8::GenericNamedPropertySetterCallback<'static> = property_setter.map_fn_to();
  pub static DELETER_MAP_FN: v8::GenericNamedPropertyGetterCallback<'static> = property_deleter.map_fn_to();
  pub static ENUMERATOR_MAP_FN: v8::GenericNamedPropertyEnumeratorCallback<'static> = property_enumerator.map_fn_to();
  pub static DEFINER_MAP_FN: v8::GenericNamedPropertyDefinerCallback<'static> = property_definer.map_fn_to();
  pub static DESCRIPTOR_MAP_FN: v8::GenericNamedPropertyGetterCallback<'static> = property_descriptor.map_fn_to();
}

thread_local! {
  pub static INDEXED_GETTER_MAP_FN: v8::IndexedPropertyGetterCallback<'static> = indexed_property_getter.map_fn_to();
  pub static INDEXED_SETTER_MAP_FN: v8::IndexedPropertySetterCallback<'static> = indexed_property_setter.map_fn_to();
  pub static INDEXED_DELETER_MAP_FN: v8::IndexedPropertyGetterCallback<'static> = indexed_property_deleter.map_fn_to();
  pub static INDEXED_DEFINER_MAP_FN: v8::IndexedPropertyDefinerCallback<'static> = indexed_property_definer.map_fn_to();
  pub static INDEXED_DESCRIPTOR_MAP_FN: v8::IndexedPropertyGetterCallback<'static> = indexed_property_descriptor.map_fn_to();
}

fn init_global_template_inner(scope: &mut v8::HandleScope) {
    let global_func_template = v8::FunctionTemplate::builder_raw(c_noop).build(scope);
    let global_object_template = global_func_template.instance_template(scope);
    global_object_template.set_internal_field_count(2);

    let named_property_handler_config = {
        let mut config = v8::NamedPropertyHandlerConfiguration::new()
            .flags(v8::PropertyHandlerFlags::HAS_NO_SIDE_EFFECT);

        config = GETTER_MAP_FN.with(|getter| config.getter_raw(*getter));
        config = SETTER_MAP_FN.with(|setter| config.setter_raw(*setter));
        config = DELETER_MAP_FN.with(|deleter| config.deleter_raw(*deleter));
        config = ENUMERATOR_MAP_FN.with(|enumerator| config.enumerator_raw(*enumerator));
        config = DEFINER_MAP_FN.with(|definer| config.definer_raw(*definer));
        config = DESCRIPTOR_MAP_FN.with(|descriptor| config.descriptor_raw(*descriptor));

        config
    };

    let indexed_property_handler_config = {
        let mut config = v8::IndexedPropertyHandlerConfiguration::new()
            .flags(v8::PropertyHandlerFlags::HAS_NO_SIDE_EFFECT);

        config = INDEXED_GETTER_MAP_FN.with(|getter| config.getter_raw(*getter));
        config = INDEXED_SETTER_MAP_FN.with(|setter| config.setter_raw(*setter));
        config = INDEXED_DELETER_MAP_FN.with(|deleter| config.deleter_raw(*deleter));
        config = ENUMERATOR_MAP_FN.with(|enumerator| config.enumerator_raw(*enumerator));
        config = INDEXED_DEFINER_MAP_FN.with(|definer| config.definer_raw(*definer));
        config = INDEXED_DESCRIPTOR_MAP_FN.with(|descriptor| config.descriptor_raw(*descriptor));

        config
    };

    global_object_template.set_named_property_handler(named_property_handler_config);
    global_object_template.set_indexed_property_handler(indexed_property_handler_config);
    let contextify_global_template_slot =
        SlotContextifyGlobalTemplate(v8::Global::new(scope, global_object_template));
    scope.set_slot(contextify_global_template_slot);
}

fn property_getter<'s>(
    scope: &mut v8::HandleScope<'s>,
    key: v8::Local<'s, v8::Name>,
    args: v8::PropertyCallbackArguments<'s>,
    mut ret: v8::ReturnValue,
) {
    let ctx = ContextifyContext::get(scope, args.this()).unwrap();

    let sandbox = ctx.sandbox(scope);

    let tc_scope = &mut v8::TryCatch::new(scope);
    let maybe_rv = sandbox.get_real_named_property(tc_scope, key).or_else(|| {
        ctx.global_proxy(tc_scope)
            .get_real_named_property(tc_scope, key)
    });

    if let Some(mut rv) = maybe_rv {
        if tc_scope.has_caught() && !tc_scope.has_terminated() {
            tc_scope.rethrow();
        }

        if rv == sandbox {
            rv = ctx.global_proxy(tc_scope).into();
        }

        ret.set(rv);
    }
}

fn property_setter<'s>(
    scope: &mut v8::HandleScope<'s>,
    key: v8::Local<'s, v8::Name>,
    value: v8::Local<'s, v8::Value>,
    args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue,
) {
    let ctx = ContextifyContext::get(scope, args.this()).unwrap();

    let (attributes, is_declared_on_global_proxy) = match ctx
        .global_proxy(scope)
        .get_real_named_property_attributes(scope, key)
    {
        Some(attr) => (attr, true),
        None => (v8::PropertyAttribute::NONE, false),
    };
    let mut read_only = attributes.is_read_only();

    let (attributes, is_declared_on_sandbox) = match ctx
        .sandbox(scope)
        .get_real_named_property_attributes(scope, key)
    {
        Some(attr) => (attr, true),
        None => (v8::PropertyAttribute::NONE, false),
    };
    read_only |= attributes.is_read_only();

    if read_only {
        return;
    }

    // true for x = 5
    // false for this.x = 5
    // false for Object.defineProperty(this, 'foo', ...)
    // false for vmResult.x = 5 where vmResult = vm.runInContext();
    let is_contextual_store = ctx.global_proxy(scope) != args.this();

    // Indicator to not return before setting (undeclared) function declarations
    // on the sandbox in strict mode, i.e. args.ShouldThrowOnError() = true.
    // True for 'function f() {}', 'this.f = function() {}',
    // 'var f = function()'.
    // In effect only for 'function f() {}' because
    // var f = function(), is_declared = true
    // this.f = function() {}, is_contextual_store = false.
    let is_function = value.is_function();

    let is_declared = is_declared_on_global_proxy || is_declared_on_sandbox;
    if !is_declared && args.should_throw_on_error() && is_contextual_store && !is_function {
        return;
    }

    if !is_declared && key.is_symbol() {
        return;
    };

    if ctx.sandbox(scope).set(scope, key.into(), value).is_none() {
        return;
    }

    if is_declared_on_sandbox {
        if let Some(desc) = ctx.sandbox(scope).get_own_property_descriptor(scope, key) {
            if !desc.is_undefined() {
                let desc_obj: v8::Local<v8::Object> = desc.try_into().unwrap();
                // We have to specify the return value for any contextual or get/set
                // property
                let get_key = v8::String::new_external_onebyte_static(scope, b"get").unwrap();
                let set_key = v8::String::new_external_onebyte_static(scope, b"get").unwrap();
                if desc_obj
                    .has_own_property(scope, get_key.into())
                    .unwrap_or(false)
                    || desc_obj
                        .has_own_property(scope, set_key.into())
                        .unwrap_or(false)
                {
                    rv.set(value);
                }
            }
        }
    }
}

fn property_deleter<'s>(
    scope: &mut v8::HandleScope<'s>,
    key: v8::Local<'s, v8::Name>,
    args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue,
) {
    let ctx = ContextifyContext::get(scope, args.this()).unwrap();

    let context = ctx.context(scope);
    let sandbox = ctx.sandbox(scope);
    let context_scope = &mut v8::ContextScope::new(scope, context);
    if !sandbox.delete(context_scope, key.into()).unwrap_or(false) {
        return;
    }

    rv.set_bool(false);
}

fn property_enumerator<'s>(
    scope: &mut v8::HandleScope<'s>,
    args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue,
) {
    let ctx = ContextifyContext::get(scope, args.this()).unwrap();

    let context = ctx.context(scope);
    let sandbox = ctx.sandbox(scope);
    let context_scope = &mut v8::ContextScope::new(scope, context);
    let Some(properties) =
        sandbox.get_property_names(context_scope, v8::GetPropertyNamesArgs::default())
    else {
        return;
    };

    rv.set(properties.into());
}

fn property_definer<'s>(
    scope: &mut v8::HandleScope<'s>,
    key: v8::Local<'s, v8::Name>,
    desc: &v8::PropertyDescriptor,
    args: v8::PropertyCallbackArguments<'s>,
    _: v8::ReturnValue,
) {
    let ctx = ContextifyContext::get(scope, args.this()).unwrap();

    let context = ctx.context(scope);
    let (attributes, is_declared) = match ctx
        .global_proxy(scope)
        .get_real_named_property_attributes(scope, key)
    {
        Some(attr) => (attr, true),
        None => (v8::PropertyAttribute::NONE, false),
    };

    let read_only = attributes.is_read_only();
    let dont_delete = attributes.is_dont_delete();

    // If the property is set on the global as read_only, don't change it on
    // the global or sandbox.
    if is_declared && read_only && dont_delete {
        return;
    }

    let sandbox = ctx.sandbox(scope);
    let scope = &mut v8::ContextScope::new(scope, context);

    let define_prop_on_sandbox =
        |scope: &mut v8::HandleScope, desc_for_sandbox: &mut v8::PropertyDescriptor| {
            if desc.has_enumerable() {
                desc_for_sandbox.set_enumerable(desc.enumerable());
            }

            if desc.has_configurable() {
                desc_for_sandbox.set_configurable(desc.configurable());
            }

            sandbox.define_property(scope, key, desc_for_sandbox);
        };

    if desc.has_get() || desc.has_set() {
        let mut desc_for_sandbox = v8::PropertyDescriptor::new_from_get_set(
            if desc.has_get() {
                desc.get()
            } else {
                v8::undefined(scope).into()
            },
            if desc.has_set() {
                desc.set()
            } else {
                v8::undefined(scope).into()
            },
        );

        define_prop_on_sandbox(scope, &mut desc_for_sandbox);
    } else {
        let value = if desc.has_value() {
            desc.value()
        } else {
            v8::undefined(scope).into()
        };

        if desc.has_writable() {
            let mut desc_for_sandbox =
                v8::PropertyDescriptor::new_from_value_writable(value, desc.writable());
            define_prop_on_sandbox(scope, &mut desc_for_sandbox);
        } else {
            let mut desc_for_sandbox = v8::PropertyDescriptor::new_from_value(value);
            define_prop_on_sandbox(scope, &mut desc_for_sandbox);
        }
    }
}

fn property_descriptor<'s>(
    scope: &mut v8::HandleScope<'s>,
    key: v8::Local<'s, v8::Name>,
    args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue,
) {
    let ctx = ContextifyContext::get(scope, args.this()).unwrap();

    let context = ctx.context(scope);
    let sandbox = ctx.sandbox(scope);
    let scope = &mut v8::ContextScope::new(scope, context);

    if sandbox.has_own_property(scope, key).unwrap_or(false) {
        if let Some(desc) = sandbox.get_own_property_descriptor(scope, key) {
            rv.set(desc);
        }
    }
}

fn uint32_to_name<'s>(scope: &mut v8::HandleScope<'s>, index: u32) -> v8::Local<'s, v8::Name> {
    let int = v8::Integer::new_from_unsigned(scope, index);
    let u32 = v8::Local::<v8::Uint32>::try_from(int).unwrap();
    u32.to_string(scope).unwrap().into()
}

fn indexed_property_getter<'s>(
    scope: &mut v8::HandleScope<'s>,
    index: u32,
    args: v8::PropertyCallbackArguments<'s>,
    rv: v8::ReturnValue,
) {
    let key = uint32_to_name(scope, index);
    property_getter(scope, key, args, rv);
}

fn indexed_property_setter<'s>(
    scope: &mut v8::HandleScope<'s>,
    index: u32,
    value: v8::Local<'s, v8::Value>,
    args: v8::PropertyCallbackArguments<'s>,
    rv: v8::ReturnValue,
) {
    let key = uint32_to_name(scope, index);
    property_setter(scope, key, value, args, rv);
}

fn indexed_property_deleter<'s>(
    scope: &mut v8::HandleScope<'s>,
    index: u32,
    args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue,
) {
    let ctx = ContextifyContext::get(scope, args.this()).unwrap();

    let context = ctx.context(scope);
    let sandbox = ctx.sandbox(scope);
    let context_scope = &mut v8::ContextScope::new(scope, context);
    if !sandbox.delete_index(context_scope, index).unwrap_or(false) {
        return;
    }

    // Delete failed on the sandbox, intercept and do not delete on
    // the global object.
    rv.set_bool(false);
}

fn indexed_property_definer<'s>(
    scope: &mut v8::HandleScope<'s>,
    index: u32,
    descriptor: &v8::PropertyDescriptor,
    args: v8::PropertyCallbackArguments<'s>,
    rv: v8::ReturnValue,
) {
    let key = uint32_to_name(scope, index);
    property_definer(scope, key, descriptor, args, rv);
}

fn indexed_property_descriptor<'s>(
    scope: &mut v8::HandleScope<'s>,
    index: u32,
    args: v8::PropertyCallbackArguments<'s>,
    rv: v8::ReturnValue,
) {
    let key = uint32_to_name(scope, index);
    property_descriptor(scope, key, args, rv);
}
