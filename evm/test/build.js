// Compiles evm/src and evm/test/mocks with solc-js and writes ABI + bytecode
// for every contract to test/out/artifacts.json.
const solc = require('solc');
const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..');
const SRC = path.join(ROOT, 'src');
const MOCKS = path.join(ROOT, 'test', 'mocks');

function walk(d, acc = []) {
  for (const f of fs.readdirSync(d)) {
    const p = path.join(d, f);
    if (fs.statSync(p).isDirectory()) walk(p, acc);
    else if (f.endsWith('.sol')) acc.push(p);
  }
  return acc;
}

const sources = {};
for (const f of [...walk(SRC), ...walk(MOCKS)]) {
  sources[path.relative(ROOT, f)] = { content: fs.readFileSync(f, 'utf8') };
}

const input = {
  language: 'Solidity',
  sources,
  settings: {
    optimizer: { enabled: true, runs: 200 },
    evmVersion: 'cancun',
    outputSelection: { '*': { '*': ['abi', 'evm.bytecode.object'] } },
  },
};

function findImport(p) {
  // imports are relative to the importing file; solc resolves them to a
  // ROOT-relative key before calling us.
  const abs = path.join(ROOT, p);
  if (fs.existsSync(abs)) return { contents: fs.readFileSync(abs, 'utf8') };
  return { error: 'not found: ' + p };
}

const out = JSON.parse(solc.compile(JSON.stringify(input), { import: findImport }));
let errors = 0;
for (const e of out.errors || []) {
  if (e.severity === 'error') { errors++; console.error(e.formattedMessage); }
}
if (errors) { console.error(`build: ${errors} error(s)`); process.exit(1); }

const artifacts = {};
for (const file of Object.keys(out.contracts)) {
  for (const name of Object.keys(out.contracts[file])) {
    const c = out.contracts[file][name];
    artifacts[name] = { abi: c.abi, bytecode: '0x' + c.evm.bytecode.object };
  }
}
fs.mkdirSync(path.join(ROOT, 'test', 'out'), { recursive: true });
fs.writeFileSync(path.join(ROOT, 'test', 'out', 'artifacts.json'), JSON.stringify(artifacts));
console.log(`build: ok (${Object.keys(artifacts).length} contracts)`);
