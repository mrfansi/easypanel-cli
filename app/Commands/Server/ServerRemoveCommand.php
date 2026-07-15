<?php

namespace App\Commands\Server;

use App\Support\ServerConfig;
use LaravelZero\Framework\Commands\Command;

class ServerRemoveCommand extends Command
{
    protected $signature = 'server:remove {name : Nama server yang dihapus}';

    protected $description = 'Hapus host EasyPanel';

    public function handle(ServerConfig $config): int
    {
        $name = $this->argument('name');

        if ($config->get($name) === null) {
            $this->error("Server '{$name}' tidak ditemukan.");

            return self::FAILURE;
        }

        $config->remove($name);
        $this->info("Server '{$name}' dihapus.");

        return self::SUCCESS;
    }
}
