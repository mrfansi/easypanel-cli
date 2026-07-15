<?php
// app/Commands/Project/ProjectCreateCommand.php
namespace App\Commands\Project;

use App\Commands\BaseServerCommand;

class ProjectCreateCommand extends BaseServerCommand
{
    protected $signature = 'project:create {name : Nama project (a-z 0-9 - _)}';

    protected $description = 'Buat project baru';

    protected function runServerCommand(): int
    {
        $name = $this->argument('name');

        if (! preg_match('/^[a-z0-9-_]+$/', $name)) {
            $this->error('Nama project hanya boleh a-z, 0-9, -, _');

            return self::FAILURE;
        }

        $this->client()->call('projects', 'createProject', ['name' => $name]);
        $this->info("Project '{$name}' dibuat.");

        return self::SUCCESS;
    }
}
