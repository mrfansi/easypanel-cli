<?php
// app/Commands/Service/ServiceStartCommand.php
namespace App\Commands\Service;

use App\Commands\BaseServerCommand;

class ServiceStartCommand extends BaseServerCommand
{
    protected $signature = 'service:start {project} {service} {--type=app : Tipe service}';

    protected $description = 'Start sebuah service';

    protected function runServerCommand(): int
    {
        $type = $this->option('type');
        $this->client()->call("services/{$type}", 'startService', [
            'projectName' => $this->argument('project'),
            'serviceName' => $this->argument('service'),
        ]);

        $this->info("Start dipicu untuk {$this->argument('project')}/{$this->argument('service')}.");

        return self::SUCCESS;
    }
}
