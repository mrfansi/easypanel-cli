<?php
// app/Commands/Project/ProjectListCommand.php
namespace App\Commands\Project;

use App\Commands\BaseServerCommand;

class ProjectListCommand extends BaseServerCommand
{
    protected $signature = 'project:list';

    protected $description = 'Daftar project di server';

    protected function runServerCommand(): int
    {
        $projects = $this->client()->call('projects', 'listProjects') ?: [];

        if ($projects === []) {
            $this->info('Tidak ada project.');

            return self::SUCCESS;
        }

        $this->table(
            ['Nama', 'Dibuat', 'Members'],
            array_map(fn ($p) => [
                $p['name'],
                $p['createdAt'] ?? '-',
                count($p['members'] ?? []),
            ], $projects),
        );

        return self::SUCCESS;
    }
}
