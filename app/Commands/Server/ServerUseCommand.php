<?php

namespace App\Commands\Server;

use App\Support\ServerConfig;
use LaravelZero\Framework\Commands\Command;

class ServerUseCommand extends Command
{
    protected $signature = 'server:use {name : Nama server yang dijadikan default}';

    protected $description = 'Jadikan sebuah server sebagai default';

    public function handle(ServerConfig $config): int
    {
        $name = $this->argument('name');

        if ($config->get($name) === null) {
            $this->error("Server '{$name}' tidak ditemukan.");

            return self::FAILURE;
        }

        $config->setDefault($name);
        $this->info("Server default sekarang: {$name}");

        return self::SUCCESS;
    }
}
