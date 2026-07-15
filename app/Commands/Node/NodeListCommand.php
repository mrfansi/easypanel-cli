<?php
// app/Commands/Node/NodeListCommand.php
namespace App\Commands\Node;

use App\Commands\BaseServerCommand;

class NodeListCommand extends BaseServerCommand
{
    protected $signature = 'node:list';

    protected $description = 'Daftar node cluster host';

    protected function runServerCommand(): int
    {
        $nodes = $this->client()->call('cluster', 'listNodes') ?: [];

        if (! is_array($nodes) || $nodes === []) {
            $this->info('Tidak ada node (atau host bukan cluster).');

            return self::SUCCESS;
        }

        $this->table(
            ['Node', 'Detail'],
            array_map(fn ($n) => is_array($n)
                ? [$n['name'] ?? $n['hostname'] ?? '-', json_encode($n)]
                : [$n, ''], $nodes),
        );

        return self::SUCCESS;
    }
}
