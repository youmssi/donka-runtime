import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { checkCoverage, checkTestLayout } from './check-rust-coverage.mjs';

const policy = { minimumLinePercent: 70, modules: ['crates/decision/src', 'crates/release/src'] };
const file = (module, covered, count = 10) => ({ filename: `/repo/${module}/lib.rs`, summary: { lines: { count, covered } } });
const report = (...files) => ({ data: [{ files }] });

test('each business crate passes independently at the floor, not after rounding or pooling', () => {
  const rows = checkCoverage(report(file(policy.modules[0], 100, 100), file(policy.modules[1], 699, 1000)), policy, '/repo');
  assert.deepEqual(rows.map((row) => row.passed), [true, false]);
  assert.equal(checkCoverage(report(file(policy.modules[0], 7)), { ...policy, modules: [policy.modules[0]] }, '/repo')[0].passed, true);
});

test('missing modules, zero executable lines and untested files cannot become passing coverage', () => {
  const rows = checkCoverage(report(file(policy.modules[0], 0, 0)), policy, '/repo');
  assert.deepEqual(rows.map((row) => row.passed), [false, false]);
  const untested = { ...file(policy.modules[0], 0, 100), filename: '/repo/crates/decision/src/untested.rs' };
  assert.equal(checkCoverage(report(file(policy.modules[0], 10), untested), policy, '/repo')[0].passed, false);
});

test('other modules do not inflate coverage and malformed exports fail closed', () => {
  assert.deepEqual(checkCoverage(report(file('vendor/crates/decision/src', 10)), policy, '/repo').map((row) => row.passed), [false, false]);
  for (const broken of [{}, { data: [] }, { data: [{ files: [] }, { files: [] }] }, report(file(policy.modules[0], 11)), report(file(policy.modules[0], -1)), report(file(policy.modules[0], 7), file(policy.modules[0], 7))]) {
    assert.throws(() => checkCoverage(broken, policy, '/repo'));
  }
  assert.throws(() => checkCoverage(report(), { ...policy, modules: [policy.modules[0], policy.modules[0]] }, '/repo'));
});

test('a thin Runtime extension can be measured as its exact source file', () => {
  const reportFile = { filename: '/repo/src/contract.rs', summary: { lines: { count: 10, covered: 7 } } };
  const rows = checkCoverage(report(reportFile), { minimumLinePercent: 70, modules: ['src/contract.rs'] }, '/repo');
  assert.equal(rows[0].passed, true);
});

test('unit-test lines cannot inflate coverage and inline test bodies fail the layout check', () => {
  const rules = { ...policy, modules: [policy.modules[0]] };
  const testFile = { ...file(policy.modules[0], 100), filename: '/repo/crates/decision/src/tests.rs' };
  assert.equal(checkCoverage(report(file(policy.modules[0], 6), testFile), rules, '/repo')[0].passed, false);
  const root = mkdtempSync(join(tmpdir(), 'donka-test-layout-'));
  try {
    const source = join(root, 'crates/decision/src');
    mkdirSync(source, { recursive: true });
    for (const inline of ['#[cfg(test)]\nmod tests { #[test] fn rule() {} }', '#[tokio::test]\nasync fn rule() {}']) {
      writeFileSync(join(source, 'lib.rs'), inline);
      assert.throws(() => checkTestLayout(rules, root), /companion tests.rs/);
    }
    writeFileSync(join(source, 'lib.rs'), '#[cfg(test)]\nmod tests;');
    writeFileSync(join(source, 'tests.rs'), '#[test]\nfn rule() {}');
    assert.doesNotThrow(() => checkTestLayout(rules, root));
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('the real command returns failure for a below-floor or incomplete module export', () => {
  const dir = mkdtempSync(join(tmpdir(), 'donka-coverage-'));
  try {
    const input = join(dir, 'report.json'), rules = join(dir, 'policy.json');
    writeFileSync(rules, JSON.stringify(policy));
    for (const module of policy.modules) {
      mkdirSync(join(dir, module), { recursive: true });
      writeFileSync(join(dir, module, 'lib.rs'), 'pub fn rule() {}');
    }
    for (const [data, status] of [[report(file(policy.modules[0], 7), file(policy.modules[1], 7)), 0], [report(file(policy.modules[0], 10), file(policy.modules[1], 6)), 1], [report(file(policy.modules[0], 10)), 1]]) {
      writeFileSync(input, JSON.stringify(data).replaceAll('/repo', dir.split('\\').join('/')));
      const result = spawnSync(process.execPath, [fileURLToPath(new URL('./check-rust-coverage.mjs', import.meta.url)), input, rules, dir], { encoding: 'utf8' });
      assert.equal(result.status, status, result.stderr);
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
