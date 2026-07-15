<?php
// app/Commands/Service/ServiceStopCommand.php
namespace App\Commands\Service;

use App\Commands\BaseServerCommand;

class ServiceStopCommand extends BaseServerCommand
{
    protected $signature = 'service:stop {project} {service} {--type=app : Tipe service}';

    protected $description = 'Stop sebuah service';

    protected function runServerCommand(): int
    {
        $type = $this->option('type');
        $this->client()->call("services/{$type}", 'stopService', [
            'projectName' => $this->argument('project'),
            'serviceName' => $this->argument('service'),
        ]);

        $this->info("Stop dipicu untuk {$this->argument('project')}/{$this->argument('service')}.");

        return self::SUCCESS;
    }
}
