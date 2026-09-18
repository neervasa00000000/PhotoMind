import assert from 'node:assert/strict';
import { planCleanup } from '../src/cleanup.ts';
const photo = (id: string, is_kept = false) => ({ id, is_kept, file_size: 10 });
const a = photo('a'), b = photo('b'), c = photo('c'), d = photo('d', true);
const groups = [
  { kind: 'exact', photos: [a, b] },
  { kind: 'similar', photos: [b, a, c, d] },
];
const all = planCleanup(groups as any);
assert.deepEqual(all.ids, ['b', 'c']);
assert.equal(all.bytes, 20);
assert.equal(all.keepers[1].photo.id, 'd');
assert.deepEqual(planCleanup(groups as any, 'exact').ids, ['b']);
assert.deepEqual(planCleanup(groups as any, 'exact').keepers.map(entry => entry.photo.id), ['a']);
assert.deepEqual(planCleanup([{kind:'similar',photos:[a,b,c]}, {kind:'similar',photos:[a,c]}] as any).ids, ['b','c']);
assert.equal(planCleanup([{kind:'similar',photos:[a,b,c]}, {kind:'similar',photos:[a,c]}] as any).bytes, 20);
assert.deepEqual(planCleanup([{kind:'exact',photos:[a,b]}, {kind:'similar',photos:[b,a,c]}] as any).ids, ['c']);
assert.deepEqual(planCleanup([]).ids, []);
console.log('Cleanup planner passed: keeper protection, group filtering, overlapping groups, unique byte counts, empty library.');
