(host, log, names) => {
  const stringify = JSON.stringify;
  const keys = Object.keys;
  const StringValue = String;
  const isArray = Array.isArray;
  const objectTag = Function.prototype.call.bind(Object.prototype.toString);
  const includes = Function.prototype.call.bind(Array.prototype.includes);
  const map = Function.prototype.call.bind(Array.prototype.map);
  const join = Function.prototype.call.bind(Array.prototype.join);
  const kind = value => value === null ? "null" : isArray(value) ? "an array" : ({undefined:"undefined",object:"an object",function:"a function",number:"a number",boolean:"a boolean",bigint:"a bigint",symbol:"a symbol",string:"a string"})[typeof value];
  const M = Map, S = Set, P = Promise, E = Error, T = TypeError, from = Array.from;
  const symbolDescription = Function.prototype.call.bind(Object.getOwnPropertyDescriptor(Symbol.prototype, "description").get);
  const format = (value, indent = "") => {
    if (typeof value === "symbol") return symbolDescription(value) ?? "";
    if (value === null || typeof value !== "object") return typeof value === "function" ? "[Function]" : StringValue(value);
    if (value instanceof E) {
      if (value.name === "GoError") return StringValue(value.message);
      const message = StringValue(value);
      const stack = value.stack ? StringValue(value.stack) : "";
      return stack && !stack.startsWith(message) ? message + "\n" + stack : stack || message;
    }
    try {
      if (value instanceof M) return "Map " + stringify(from(value));
      if (value instanceof S) return "Set " + stringify(from(value));
      if (value instanceof P) return "[a Promise: await it for its value]";
      return stringify(value, null, indent) ?? "[Object]";
    } catch (error) { return "[Object, not JSON: " + StringValue(error) + "]"; }
  };
  const string = (value, fn, label) => {
    if (typeof value !== "string") throw new T(fn + ": " + label + " must be a string, not " + kind(value));
    return value;
  };
  const object = (value, fn, label) => {
    if (value === null || typeof value !== "object" || isArray(value) || objectTag(value) !== "[object Object]") throw new T(fn + ": the " + label + " must be an object, not " + kind(value));
    return value;
  };
  const options = (value, fn, known) => {
    if (value == null) return {};
    object(value, fn, "options");
    for (const key of keys(value)) if (!includes(known, key)) throw new T(fn + ": unknown option " + stringify(key) + "; the options are " + join(known, ", "));
    return value;
  };
  const console = Object.create(null);
  for (const level of ["log", "info", "warn", "error", "debug"]) console[level] = (...args) => log(join(map(args, value => format(value)), " ") + "\n");
  globalThis.console = console;
  const number = Number, finite = Number.isFinite, create = Object.create;
  const numericOptions = (value, fn, known) => {
    const input = options(value, fn, known), output = create(null);
    for (const key of known) {
      const value = input[key];
      if (value == null) continue;
      const n = number(value);
      output[key] = finite(n) ? n : StringValue(n);
    }
    return output;
  };
  globalThis.sleep = milliseconds => host("", stringify([number(milliseconds)]));
  globalThis.bash = (command, opts) => host("bash", stringify([string(command, "bash", "the command"), numericOptions(opts, "bash", ["timeout", "maxOutputLength"])]));
  globalThis.read = (path, opts) => host("read", stringify([string(path, "read", "the path"), numericOptions(opts, "read", ["offset", "limit"])]));
  for (const name of ["readText", "skill", "viewImage"]) globalThis[name] = path => host(name, stringify([string(path, name, name === "skill" ? "the name" : "the path")]));
  globalThis.write = (path, content) => host("write", stringify([string(path, "write", "the path"), string(content, "write", "the content")]));
  globalThis.edit = (path, oldString, newString, opts) => host("edit", stringify([string(path, "edit", "the path"), string(oldString, "edit", "oldString"), string(newString, "edit", "newString"), {replaceAll: !!options(opts, "edit", ["replaceAll"]).replaceAll}]));
  globalThis.applyPatch = patch => host("applyPatch", stringify([string(patch, "applyPatch", "the patch")]));
  globalThis.transcriptSearch = (query, opts) => host("transcriptSearch", stringify([string(query, "transcriptSearch", "the query"), options(opts, "transcriptSearch", ["limit"])]));
  const builtins = ["bash","read","readText","write","edit","applyPatch","viewImage","transcriptSearch","sleep","skill","console","tools"];
  const reserved = new Set("await break case catch class const continue debugger default delete do else enum export extends false finally for function if implements import in instanceof interface let new null package private protected public return static super switch this throw true try typeof var void while with yield".split(" "));
  const tools = Object.create(null);
  for (const name of names) {
    const fn = args => {
      args ??= {};
      object(args, name, "arguments");
      let encoded;
      try { encoded = stringify(args); }
      catch (error) { throw new T(name + ": the arguments must be JSON: " + StringValue(error)); }
      return host(name, encoded, true);
    };
    tools[name] = fn;
    if (/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name) && !reserved.has(name) && !includes(builtins, name) && !(name in globalThis)) globalThis[name] = fn;
  }
  globalThis.tools = tools;
  return { format, thrown: value => value instanceof E || typeof value === "string" ? format(value) : "uncaught " + format(value) };
}
