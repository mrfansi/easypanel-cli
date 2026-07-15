<?php
// app/Commands/Service/ServiceLogsCommand.php
namespace App\Commands\Service;

use App\Commands\BaseServerCommand;

class ServiceLogsCommand extends BaseServerCommand
{
    protected $signature = 'service:logs {project} {service} {--limit=100 : Jumlah baris log}';

    protected $description = 'Tampilkan log service';

    protected function runServerCommand(): int
    {
        $result = $this->client()->call('logs', 'queryServiceLogs', [
            'projectName' => $this->argument('project'),
            'serviceName' => $this->argument('service'),
            'limit' => (int) $this->option('limit'),
        ]) ?: [];

        $rows = [];

        // Bentuk Loki: { entries: [ { values: [ [ns_timestamp, message], ... ] } ] }
        if (isset($result['entries'])) {
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

        if ($rows === []) {
            $this->info('Tidak ada log.');

            return self::SUCCESS;
        }

        // Urut terlama -> terbaru (timestamp nanodetik sebagai string).
        usort($rows, fn ($a, $b) => (string) $a[0] <=> (string) $b[0]);

        foreach ($rows as [$ts, $message]) {
            $time = $ts !== null ? date('H:i:s', (int) ((int) $ts / 1_000_000_000)).' ' : '';
            $this->line($time.$message);
        }

        return self::SUCCESS;
    }
}
