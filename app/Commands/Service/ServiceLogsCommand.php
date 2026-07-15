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
        $lines = $this->client()->call('logs', 'queryServiceLogs', [
            'projectName' => $this->argument('project'),
            'serviceName' => $this->argument('service'),
            'limit' => (int) $this->option('limit'),
        ]) ?: [];

        // Bentuk log bisa berupa list string atau list objek; tangani keduanya.
        foreach ((is_array($lines) ? $lines : [$lines]) as $line) {
            if (is_string($line)) {
                $this->line($line);
            } elseif (is_array($line)) {
                $this->line($line['message'] ?? $line['line'] ?? json_encode($line));
            }
        }

        return self::SUCCESS;
    }
}
