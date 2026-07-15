<?php
// app/Support/LogFormatter.php
namespace App\Support;

class LogFormatter
{
    /**
     * Ubah respons queryServiceLogs jadi baris siap-tampil, terurut terlama -> terbaru.
     *
     * @return list<string>
     */
    public static function format(mixed $result): array
    {
        $rows = [];

        // Bentuk Loki: { entries: [ { values: [ [ns_timestamp, message], ... ] } ] }
        if (is_array($result) && isset($result['entries'])) {
            foreach ($result['entries'] as $entry) {
                foreach ($entry['values'] ?? [] as [$ts, $message]) {
                    $rows[] = [$ts, $message];
                }
            }
        } else {
            // Fallback: list string atau list objek.
            foreach ((array) $result as $line) {
                $rows[] = [null, is_string($line) ? $line : ($line['message'] ?? $line['line'] ?? json_encode($line))];
            }
        }

        usort($rows, fn ($a, $b) => (string) $a[0] <=> (string) $b[0]);

        return array_map(function ($row) {
            [$ts, $message] = $row;
            $time = $ts !== null ? date('H:i:s', (int) ((int) $ts / 1_000_000_000)).' ' : '';

            return $time.$message;
        }, $rows);
    }
}
