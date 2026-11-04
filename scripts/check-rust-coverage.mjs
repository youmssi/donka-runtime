import { readFileSync, readdirSync, statSync } from 'node:fs';
import { resolve, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const testSource = /(?:^|\/)tests(?:\/|\.rs$)|(?:_|-)tests\.rs$/;

/** Stable coverage must measure production lines, not inline unit-test bodies. */
export function checkTestLayout(policy, root) {
  function visit(path) {
    if (statSync(path).isDirectory()) {
      for (const child of readdirSync(path)) visit(resolve(path, child));
      return;
    }
    const name = relative(root, path).split(sep).join('/');
    if (!name.endsWith('.rs') || testSource.test(name)) return;
    const source = readFileSync(path, 'utf8');
    if (/^\s*#\[cfg\(test\)\]\s*(?:#\[[^\]]*\]\s*)*mod\s+\w+\s*\{/m.test(source) ||
        /^\s*#\[(?:(?:tokio|sqlx)::)?test(?:\]|\()/m.test(source)) {
      throw new Error(`Move unit-test bodies to a companion tests.rs file: ${name}`);
    }
  }
  for (const module of policy.modules) visit(resolve(root, module));
}

/** Require measured executable lines in every named business module, including untested files. */
export function checkCoverage(report, policy, root) {
  const minimum = policy.minimumLinePercent;
  if (!Number.isFinite(minimum) || minimum <= 0 || minimum > 100 || !policy.modules?.length) {
    throw new Error('Invalid coverage policy');
  }
  if (!Array.isArray(report.data) || report.data.length !== 1 || !Array.isArray(report.data[0].files)) {
    throw new Error('Expected one LLVM coverage export');
  }
  const modules = new Map();
  for (const module of policy.modules) {
    if (typeof module !== 'string' || module.includes('..') || modules.has(module)) {
      throw new Error('Invalid or duplicate coverage module');
    }
    modules.set(module, { covered: 0, count: 0 });
  }
  const seen = new Set();
  for (const file of report.data[0].files) {
    const name = relative(resolve(root), resolve(file.filename)).split(sep).join('/');
    if (seen.has(name)) throw new Error(`Duplicate coverage file: ${name}`);
    seen.add(name);
    if (testSource.test(name)) continue;
    const module = [...modules.keys()].find((prefix) => name === prefix || name.startsWith(`${prefix}/`));
    if (!module) continue;
    const lines = file.summary?.lines;
    if (
      !Number.isSafeInteger(lines?.count) || !Number.isSafeInteger(lines?.covered) ||
      lines.count < 0 || lines.covered < 0 || lines.covered > lines.count
    ) throw new Error(`Invalid line counts: ${name}`);
    const total = modules.get(module);
    total.count += lines.count;
    total.covered += lines.covered;
  }
  return [...modules].map(([module, lines]) => ({
    module,
    ...lines,
    percent: lines.count ? 100 * lines.covered / lines.count : 0,
    passed: lines.count > 0 && 100 * lines.covered >= minimum * lines.count,
  }));
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const [reportFile, policyFile, root = process.cwd()] = process.argv.slice(2);
    if (!reportFile || !policyFile) throw new Error('usage: check-rust-coverage.mjs <llvm.json> <policy.json> [root]');
    const policy = JSON.parse(readFileSync(policyFile, 'utf8'));
    const results = checkCoverage(JSON.parse(readFileSync(reportFile, 'utf8')), policy, root);
    checkTestLayout(policy, root);
    for (const row of results) console.log(`${row.module}: ${row.covered}/${row.count} lines (${row.percent.toFixed(2)}%) ${row.passed ? 'PASS' : 'FAIL'}`);
    if (results.some((row) => !row.passed)) process.exitCode = 1;
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
