// Copyright 2018-2024 the Deno authors. All rights reserved. MIT license.

// deno-lint-ignore-file no-explicit-any

import { notImplemented } from "ext:deno_node/_utils.ts";

export class Script {
  #inner;

  constructor(code: string, _options = {}) {
  }

  runInThisContext(_options: any) {
    notImplemented("Script.prototype.runInThisContext");
  }

  runInContext(contextifiedObject: any, _options: any) {
    notImplemented("Script.prototype.runInContext");
  }

  runInNewContext(contextObject: any, options: any) {
    notImplemented("Script.prototype.vm");
  }

  createCachedData() {
    notImplemented("Script.prototype.createCachedData");
  }
}

export function createContext(contextObject: any = {}, _options: any) {
  if (isContext(contextObject)) {
    return contextObject;
  }

  op_vm_create_context(contextObject);
  return contextObject;
}

export function createScript(code: string, options: any) {
  return new Script(code, options);
}

export function runInContext(
  code: string,
  contextifiedObject: any,
  _options: any,
) {
  return createScript(code).runInContext(contextifiedObject);
}

export function runInNewContext(
  code: string,
  contextObject: any,
  options: any,
) {
  if (options) {
    console.warn("vm.runInNewContext options are currently not supported");
  }
  return createScript(code).runInNewContext(contextObject);
}

export function runInThisContext(
  code: string,
  options: any,
) {
  return createScript(code, options).runInThisContext(options);
}

export function isContext(maybeContext: any) {
  return false;
}

export function compileFunction(_code: string, _params: any, _options: any) {
  notImplemented("compileFunction");
}

export function measureMemory(_options: any) {
  notImplemented("measureMemory");
}

export default {
  Script,
  createContext,
  createScript,
  runInContext,
  runInNewContext,
  runInThisContext,
  isContext,
  compileFunction,
  measureMemory,
};
