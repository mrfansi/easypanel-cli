<?php
// app/Commands/BaseServerCommand.php
namespace App\Commands;

use App\Support\EasypanelClient;
use App\Support\EasypanelException;
use App\Support\ServerConfig;
use LaravelZero\Framework\Commands\Command;
use Symfony\Component\Console\Input\InputOption;

abstract class BaseServerCommand extends Command
{
    protected function configure(): void
    {
        parent::configure();
        $this->addOption('server', null, InputOption::VALUE_REQUIRED, 'Nama server target (default: server default)');
    }

    public function handle(): int
    {
        try {
            return $this->runServerCommand();
        } catch (EasypanelException $e) {
            $this->error($e->getMessage());

            return self::FAILURE;
        }
    }

    abstract protected function runServerCommand(): int;

    protected function client(): EasypanelClient
    {
        $config = app(ServerConfig::class);
        $name = $this->option('server');
        $server = $name ? $config->get($name) : $config->default();

        if ($server === null) {
            throw new EasypanelException($name
                ? "Server '{$name}' tidak ditemukan. Lihat: easypanel server:list"
                : 'Belum ada server default. Jalankan: easypanel server:add');
        }

        return new EasypanelClient($server['url'], $server['token']);
    }
}
