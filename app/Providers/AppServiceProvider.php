<?php

namespace App\Providers;

use App\Support\ServerConfig;
use Illuminate\Support\ServiceProvider;

class AppServiceProvider extends ServiceProvider
{
    /**
     * Bootstrap any application services.
     */
    public function boot(): void
    {
        //
    }

    /**
     * Register any application services.
     */
    public function register(): void
    {
        $this->app->singleton(ServerConfig::class, fn () => new ServerConfig(ServerConfig::defaultPath()));
    }
}
