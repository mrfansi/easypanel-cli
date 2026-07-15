<?php
// app/Commands/Service/ServiceDeployCommand.php
namespace App\Commands\Service;

use App\Commands\BaseServerCommand;

class ServiceDeployCommand extends BaseServerCommand
{
    protected $signature = 'service:deploy {project} {service} {--type=app : Tipe service} {--force : Force rebuild}';

    protected $description = 'Deploy sebuah service';

    protected function runServerCommand(): int
    {
        $type = $this->option('type');
        $this->client()->call("services/{$type}", 'deployService', [
            'projectName' => $this->argument('project'),
            'serviceName' => $this->argument('service'),
            'forceRebuild' => (bool) $this->option('force'),
        ]);

        $this->info("Deploy dipicu untuk {$this->argument('project')}/{$this->argument('service')}.");

        return self::SUCCESS;
    }
}
