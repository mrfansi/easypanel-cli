<?php
// tests/Unit/LogFormatterTest.php
use App\Support\LogFormatter;

it('flattens Loki entries oldest-first with a HH:MM:SS prefix', function () {
    // 1_600_000_000s and 1_600_000_060s (60s apart) in nanoseconds.
    $result = ['entries' => [[
        'values' => [
            ['1600000060000000000', 'baris kedua'],
            ['1600000000000000000', 'baris pertama'],
        ],
    ]]];

    $lines = LogFormatter::format($result);

    expect($lines)->toHaveCount(2);
    // Oldest first.
    expect($lines[0])->toEndWith('baris pertama');
    expect($lines[1])->toEndWith('baris kedua');
    // Prefixed with a HH:MM:SS timestamp.
    expect($lines[0])->toMatch('/^\d{2}:\d{2}:\d{2} baris pertama$/');
});

it('falls back to plain strings when the shape is not Loki', function () {
    expect(LogFormatter::format(['satu', 'dua']))->toBe(['satu', 'dua']);
});

it('returns an empty array for empty input', function () {
    expect(LogFormatter::format([]))->toBe([]);
});
