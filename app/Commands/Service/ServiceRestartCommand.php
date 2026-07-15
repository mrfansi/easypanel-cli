<?php
// app/Commands/Service/ServiceRestartCommand.php
namespace App\Commands\Service;

use App\Commands\BaseServerCommand;

class ServiceRestartCommand extends BaseServerCommand
{
    protected $signature = 'service:restart {project} {service} {--type=app : Tipe service}';

    protected $description = 'Restart sebuah service';

    protected function runServerCommand(): int
    {
        $type = $this->option('type');
        $this->client()->call("services/{$type}", 'restartService', [
            'projectName' => $this->argument('project'),
            'serviceName' => $this->argument('service'),
        ]);

        $this->info("Restart dipicu untuk {$this->argument('project')}/{$this->argument('service')}.");

        return self::SUCCESS;
    }
}
