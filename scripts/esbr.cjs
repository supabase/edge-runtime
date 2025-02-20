#!/usr/bin/env node

const zlib = require("zlib");
const fs = require("fs");
const path = require("path");
const process = require("process");

const command = process.argv[2];
let filename = process.argv[3];

if (command === void 0) {
  console.error("no command is specified");
  process.exit(1);
}

if (!path.isAbsolute(filename)) {
  filename = path.join(process.cwd(), filename);
}

if (filename === void 0) {
  console.error("no path is specified");
  process.exit(1);
}

try {
  if (fs.lstatSync(filename).isDirectory()) {
    console.error(`${filename} is a directory`);
    process.exit(1);
  }
} catch (ex) {
  console.error(`could not read file from given path: ${ex.toString()}`);
  process.exit(1);
}

if (command === "compress") {
  /** @type {Uint8Array} */
  const buf = fs.readFileSync(filename);

  const ezbr = new TextEncoder().encode("EZBR");
  const compBuf = zlib.brotliCompressSync(buf);

  fs.writeFileSync(`${filename}.out`, ezbr);
  fs.appendFileSync(`${filename}.out`, compBuf);
} else if (command === "decompress") {
  /** @type {Uint8Array} */
  let buf = fs.readFileSync(filename);

  if (buf.length >= 4 && new TextDecoder().decode(buf.slice(0, 4)) === "EZBR") {
    buf = buf.slice(4);
  } else {
    console.error("malformed data");
    process.exit(1);
  }

  fs.writeFileSync(`${filename}.out`, zlib.brotliDecompressSync(buf));
} else {
  console.error(`invalid command: ${command}`);
  process.exit(1);
}

console.log(`written in ${filename}.out`);
