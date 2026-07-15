<?php
// app/Commands/Service/ServiceLogsCommand.php
namespace App\Commands\Service;

use App\Commands\BaseServerCommand;
use App\Support\LogFormatter;

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

        $lines = LogFormatter::format($result);

        if ($lines === []) {
            $this->info('Tidak ada log.');

            return self::SUCCESS;
        }

        foreach ($lines as $line) {
            $this->line($line);
        }

        return self::SUCCESS;
    }
}
