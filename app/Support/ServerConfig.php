<?php
// app/Support/ServerConfig.php
namespace App\Support;

class ServerConfig
{
    public function __construct(private string $path) {}

    public static function defaultPath(): string
    {
        $home = getenv('HOME') ?: getenv('USERPROFILE') ?: sys_get_temp_dir();

        return $home.'/.config/easypanel/servers.json';
    }

    /** @return array<int, array{name:string,url:string,token:string,default:bool}> */
    public function all(): array
    {
        if (! is_file($this->path)) {
            return [];
        }

        return json_decode((string) file_get_contents($this->path), true) ?: [];
    }

    public function get(string $name): ?array
    {
        foreach ($this->all() as $server) {
            if ($server['name'] === $name) {
                return $server;
            }
        }

        return null;
    }

    public function default(): ?array
    {
        foreach ($this->all() as $server) {
            if ($server['default'] ?? false) {
                return $server;
            }
        }

        return null;
    }

    public function add(string $name, string $url, string $token): void
    {
        $wasDefault = $this->get($name)['default'] ?? false;
        $servers = array_values(array_filter($this->all(), fn ($s) => $s['name'] !== $name));

        $servers[] = [
            'name' => $name,
            'url' => $url,
            'token' => $token,
            // Default bila: server pertama, ATAU server ini yang tadinya default
            // (mis. rotasi token), ATAU tak ada server lain yang bertanda default.
            'default' => $servers === [] || $wasDefault || ! $this->hasDefault($servers),
        ];

        $this->save($servers);
    }

    public function remove(string $name): void
    {
        $servers = array_values(array_filter($this->all(), fn ($s) => $s['name'] !== $name));

        if ($servers !== [] && ! $this->hasDefault($servers)) {
            $servers[0]['default'] = true;
        }

        $this->save($servers);
    }

    public function setDefault(string $name): void
    {
        $servers = array_map(function ($s) use ($name) {
            $s['default'] = $s['name'] === $name;

            return $s;
        }, $this->all());

        $this->save($servers);
    }

    private function hasDefault(array $servers): bool
    {
        foreach ($servers as $s) {
            if ($s['default'] ?? false) {
                return true;
            }
        }

        return false;
    }

    private function save(array $servers): void
    {
        $dir = dirname($this->path);
        if (! is_dir($dir)) {
            mkdir($dir, 0700, true);
        }

        file_put_contents($this->path, json_encode($servers, JSON_PRETTY_PRINT));
        chmod($this->path, 0600);
    }
}
