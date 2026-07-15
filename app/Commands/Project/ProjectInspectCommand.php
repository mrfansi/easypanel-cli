<?php
// app/Commands/Project/ProjectInspectCommand.php
namespace App\Commands\Project;

use App\Commands\BaseServerCommand;

class ProjectInspectCommand extends BaseServerCommand
{
    protected $signature = 'project:inspect {name : Nama project}';

    protected $description = 'Lihat detail project dan service-nya';

    protected function runServerCommand(): int
    {
        $data = $this->client()->call('projects', 'inspectProject', ['projectName' => $this->argument('name')]);

        $services = $data['services'] ?? [];

        if ($services === []) {
            $this->info('Project tidak punya service.');

            return self::SUCCESS;
        }

        $this->table(
            ['Service', 'Tipe', 'Aktif'],
            array_map(fn ($s) => [
                $s['name'],
                $s['type'] ?? '-',
                ($s['enabled'] ?? false) ? 'ya' : 'tidak',
            ], $services),
        );

        return self::SUCCESS;
    }
}
